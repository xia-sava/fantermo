use crate::common::Server;
use crate::common::processes;
use crate::common::processes::Process;
use crate::ui::{
    CMD_COMMAND, CMD_LONG_COMMAND, CMD_LONG_PROCESS, SH_LONG_COMMAND, cygwin_config,
    cygwin_sleep_is_running, tab_count, unique_sleep_seconds, wait_cmd_ready, wait_cygwin_sleep,
    wait_sh_ready, wsl_config,
};
use std::thread;
use std::time::Duration;
use std::time::Instant;

/// 長く動くコマンドを動かしているタブ 2 つと、何も動かしていないタブ 1 つでウィンドウを閉じると、子孫のプロセスが
/// 残らない。`wait_ready` はタブのシェルが入力を受け付けるのを待つ。
fn exercise_closing_the_window(
    mut server: Server,
    long_commands: [&str; 2],
    wait_long_commands: &dyn Fn(&Server),
    wait_ready: fn(&Server),
) {
    wait_ready(&server);
    server.send_line(long_commands[0]);
    server.post("/e2e/tab/new", b"");
    server.wait_until("two tabs", |state| tab_count(state) == 2);
    wait_ready(&server);
    server.send_line(long_commands[1]);
    server.post("/e2e/tab/new", b"");
    server.wait_until("three tabs", |state| tab_count(state) == 3);
    wait_ready(&server);

    wait_long_commands(&server);
    server.post("/e2e/quit", b"");
    assert_eq!(server.wait_exit(), 0);
    server.assert_no_children_remained();
}

/// 一行で続けて起動する、同じコンソールの `ping` の数。
const CONSOLE_CLIENTS: u32 = 20;
/// 接続の途中で閉じるタブの数。
const TABS_CLOSED_WHILE_STARTING: usize = 6;
/// `start /b` で起動したプロセスができるのを待つ上限。
const CONSOLE_CLIENT_TIMEOUT: Duration = Duration::from_secs(60);
/// コンソールの外のプロセスができたかを確認する間隔。
const CONSOLE_CLIENT_POLL_INTERVAL: Duration = Duration::from_millis(50);
/// 新しいプロセスができたのを見つけるまでの間隔。できるだけ早く見つけたいが、スナップショットを間を置かずに取り
/// 続けて CPU を占有しないよう、短く置く。
const NEW_DESCENDANT_POLL_INTERVAL: Duration = Duration::from_millis(10);

/// どのコンソールにも接続せずに動き続ける、窓を出さないプログラム。
const GUI_PROCESS: &str = "wscript.exe";
/// 閉じたあと、起動の途中だった Cygwin の `sleep` が `/proc` に見えるようになるまで置く時間。
const CYGWIN_SLEEP_SETTLE: Duration = Duration::from_secs(2);

#[test]
fn closing_the_window_ends_the_child_processes_of_every_tab() {
    let server = Server::start_watching_children("", &["cmd.exe", "/k", "prompt", "e2e$G"]);
    let wait_pings = |server: &Server| {
        processes::wait_descendants_named(server.pid(), CMD_LONG_PROCESS, 2);
    };
    exercise_closing_the_window(
        server,
        [CMD_LONG_COMMAND, CMD_LONG_COMMAND],
        &wait_pings,
        wait_cmd_ready,
    );
}

/// 長く動くコマンドを動かしているタブを閉じると、そのタブの子孫のプロセスが終わり、ほかのタブのものは動き続ける。
#[test]
fn closing_a_tab_ends_its_child_processes_only() {
    let server = Server::start_cmd();
    wait_cmd_ready(&server);
    let pid = server.pid();
    let first = processes::descendants(pid);

    server.post("/e2e/tab/new", b"");
    server.wait_until("two tabs", |state| tab_count(state) == 2);
    wait_cmd_ready(&server);
    server.type_line(CMD_LONG_COMMAND);
    // プロセスができたらすぐ閉じる。間を空けて見ると、接続を終えたあとに閉じることが多くなる。
    let second = wait_new_descendant(pid, CMD_LONG_PROCESS, &first);

    server.post("/e2e/tab/close", b"");
    server.wait_until("one tab", |state| tab_count(state) == 1);
    processes::wait_ended(&second);
    processes::assert_running(&first);
}

/// 疑似コンソールを閉じる前後に接続するプロセスができ続けるよう、同じコンソールの `ping` を続けて起動する行。
fn cmd_line_starting_console_clients() -> String {
    format!("for /l %i in (1,1,{CONSOLE_CLIENTS}) do @start /b {CMD_LONG_COMMAND} >NUL")
}

/// `name` のプロセスが `root` の子孫に新しくできるのを、できるだけ間を空けずに待つ。
#[track_caller]
fn wait_new_descendant(root: u32, name: &str, known: &[Process]) -> Vec<Process> {
    let deadline = Instant::now() + CONSOLE_CLIENT_TIMEOUT;
    loop {
        let found = processes::descendants(root);
        let new: Vec<Process> = found
            .into_iter()
            .filter(|process| !known.contains(process))
            .collect();
        if new.iter().any(|process| process.name == name) {
            return new;
        }
        assert!(Instant::now() < deadline, "no new {name} under {root}");
        thread::sleep(NEW_DESCENDANT_POLL_INTERVAL);
    }
}

/// 新しいタブを開いてシェルの準備ができるのを待ち、タブのプロセス（疑似コンソールの `conhost.exe` とシェル）を返す。
#[track_caller]
fn open_cmd_tab(server: &Server) -> Vec<Process> {
    let before = processes::descendants(server.pid());
    let tabs = tab_count(&server.state());
    server.post("/e2e/tab/new", b"");
    server.wait_until("a new tab", |state| tab_count(state) == tabs + 1);
    wait_cmd_ready(server);
    processes::descendants(server.pid())
        .into_iter()
        .filter(|process| !before.contains(process))
        .collect()
}

/// 接続の途中のプロセスがあるタブを閉じても、そのタブの疑似コンソールのプロセス（`conhost.exe`）まで終わる。
/// `conhost.exe` は接続しているプロセスがなくなるまで終わらないので、親を辿れなくなったものも含めて残っていないと分かる。
#[test]
fn closing_tabs_while_commands_start_ends_their_child_processes() {
    let server = Server::start_cmd();
    wait_cmd_ready(&server);
    let pid = server.pid();
    let first = processes::descendants(pid);

    let mut closed = Vec::new();
    for _ in 0..TABS_CLOSED_WHILE_STARTING {
        let tab = open_cmd_tab(&server);
        let known = processes::descendants(pid);
        server.send_line(&cmd_line_starting_console_clients());
        closed.extend(wait_new_descendant(pid, CMD_LONG_PROCESS, &known));
        closed.extend(tab);
        server.post("/e2e/tab/close", b"");
        server.wait_until("one tab", |state| tab_count(state) == 1);
    }

    processes::wait_ended(&closed);
    processes::assert_running(&first);
}

/// 接続の途中のプロセスがあるままウィンドウを閉じても、子孫のプロセスは残らない。
#[test]
fn closing_the_window_while_commands_start_leaves_no_child_processes() {
    let mut server = Server::start_watching_children("", &CMD_COMMAND);
    wait_cmd_ready(&server);
    open_cmd_tab(&server);
    let known = processes::descendants(server.pid());
    server.send_line(&cmd_line_starting_console_clients());
    server.post("/e2e/tab/select/0", b"");
    server.send_line(&cmd_line_starting_console_clients());
    wait_new_descendant(server.pid(), CMD_LONG_PROCESS, &known);
    server.post("/e2e/quit", b"");
    assert_eq!(server.wait_exit(), 0);
    server.assert_no_children_remained();
}

/// `root` の子孫に新しくできる `ping.exe` と窓のないプログラムを待って返す。
#[track_caller]
fn wait_descendants_outside_the_console(root: u32, known: &[Process]) -> Vec<Process> {
    let deadline = Instant::now() + CONSOLE_CLIENT_TIMEOUT;
    loop {
        let found: Vec<Process> = processes::descendants(root)
            .into_iter()
            .filter(|process| !known.contains(process))
            .filter(|process| process.name == CMD_LONG_PROCESS || process.name == GUI_PROCESS)
            .collect();
        if found.iter().any(|process| process.name == GUI_PROCESS)
            && found.iter().any(|process| process.name == CMD_LONG_PROCESS)
        {
            return found;
        }
        assert!(Instant::now() < deadline, "under {root}: {found:#?}");
        thread::sleep(CONSOLE_CLIENT_POLL_INTERVAL);
    }
}

/// 別のコンソールを持つプロセスと、どのコンソールにも接続しないプロセスは、接続の途中のプロセスがあったタブを
/// 閉じても動き続ける。
#[test]
fn closing_a_tab_while_commands_start_leaves_processes_outside_its_console() {
    let server = Server::start_cmd();
    wait_cmd_ready(&server);
    let pid = server.pid();
    let script = server.dir().join("sleep.vbs");
    std::fs::write(&script, "WScript.Sleep 300000\n").expect("failed to write the script");
    let tab = open_cmd_tab(&server);
    let known = processes::descendants(pid);
    // 親の PowerShell を動かしたままにして、親子で辿れるようにする。窓のあるコンソールは、別デスクトップで起動しても
    // 既定の端末へ渡って利用者の画面に出ることがあるので、窓のないコンソール（`CreateNoWindow`）で起動する。
    server.send_line(&format!(
        "start /b powershell.exe -NoProfile -Command \"$i = New-Object Diagnostics.ProcessStartInfo 'ping.exe', '-t 127.0.0.1'; $i.UseShellExecute = $false; $i.CreateNoWindow = $true; [void][Diagnostics.Process]::Start($i); Start-Sleep 300\" & start \"\" {GUI_PROCESS} //B \"{}\"",
        script.display()
    ));
    let detached = wait_descendants_outside_the_console(pid, &known);
    assert_eq!(detached.len(), 2, "{detached:#?}");

    let known = processes::descendants(pid);
    server.send_line(&cmd_line_starting_console_clients());
    wait_new_descendant(pid, CMD_LONG_PROCESS, &known);
    server.post("/e2e/tab/close", b"");
    server.wait_until("one tab", |state| tab_count(state) == 1);

    processes::wait_ended(&tab);
    processes::assert_running(&detached);
}

/// `sleep` を打った直後に閉じるときの、打ってから閉じるまでの時間（ミリ秒）。シェルが SIGHUP を受け取るのがコマンドの
/// 起動の途中になるよう、タブごとに変える。
const CYGWIN_CLOSE_DELAYS_MS: [u64; 6] = [10, 20, 30, 40, 50, 70];

/// 動き出すのを待たずにタブを閉じても、コマンドは残らない。
#[test]
fn cygwin_closing_tabs_right_after_typing_a_command_ends_it() {
    let Some(config) = cygwin_config("cygwin_closing_tabs_right_after_typing") else {
        return;
    };
    let server = Server::start(&config, &[]);
    wait_sh_ready(&server);
    let mut markers = Vec::new();
    for delay in CYGWIN_CLOSE_DELAYS_MS {
        server.post("/e2e/tab/new", b"");
        server.wait_until("two tabs", |state| tab_count(state) == 2);
        wait_sh_ready(&server);
        let marker = unique_sleep_seconds();
        server.send_line(&format!("sleep {marker}"));
        thread::sleep(Duration::from_millis(delay));
        server.post("/e2e/tab/close", b"");
        server.wait_until("one tab", |state| tab_count(state) == 1);
        markers.push(marker);
    }
    // 起動の途中だった `sleep` が `/proc` に見えるようになるまで待ってから見る。
    thread::sleep(CYGWIN_SLEEP_SETTLE);
    for marker in &markers {
        wait_cygwin_sleep(marker, false);
    }
}

/// 動き出すのを待たずにウィンドウを閉じても、コマンドは残らない。ブリッジの標準入力と標準エラー出力が同時に閉じる。
#[test]
fn cygwin_closing_the_window_right_after_typing_a_command_ends_it() {
    let Some(config) = cygwin_config("cygwin_closing_the_window_right_after_typing") else {
        return;
    };
    for delay in CYGWIN_CLOSE_DELAYS_MS {
        let mut server = Server::start_watching_children(&config, &[]);
        wait_sh_ready(&server);
        server.send_line(&format!("sleep {}", unique_sleep_seconds()));
        thread::sleep(Duration::from_millis(delay));
        server.post("/e2e/quit", b"");
        assert_eq!(server.wait_exit(), 0);
        server.assert_no_children_remained();
    }
}

#[test]
fn cygwin_closing_the_window_ends_the_child_processes_of_every_tab() {
    let Some(config) = cygwin_config("cygwin_closing_the_window") else {
        return;
    };
    let server = Server::start_watching_children(&config, &[]);
    let (first, second) = (unique_sleep_seconds(), unique_sleep_seconds());
    let wait_sleeps = |_: &Server| {
        wait_cygwin_sleep(&first, true);
        wait_cygwin_sleep(&second, true);
    };
    exercise_closing_the_window(
        server,
        [&format!("sleep {first}"), &format!("sleep {second}")],
        &wait_sleeps,
        wait_sh_ready,
    );
    wait_cygwin_sleep(&first, false);
    wait_cygwin_sleep(&second, false);
}

#[test]
fn cygwin_closing_a_tab_ends_its_child_processes_only() {
    let Some(config) = cygwin_config("cygwin_closing_a_tab") else {
        return;
    };
    let server = Server::start(&config, &[]);
    wait_sh_ready(&server);
    let first = unique_sleep_seconds();
    server.send_line(&format!("sleep {first}"));
    wait_cygwin_sleep(&first, true);

    server.post("/e2e/tab/new", b"");
    server.wait_until("two tabs", |state| tab_count(state) == 2);
    wait_sh_ready(&server);
    let second = unique_sleep_seconds();
    server.send_line(&format!("sleep {second}"));
    wait_cygwin_sleep(&second, true);

    server.post("/e2e/tab/close", b"");
    server.wait_until("one tab", |state| tab_count(state) == 1);
    wait_cygwin_sleep(&second, false);
    assert!(cygwin_sleep_is_running(&first));
}

/// WSL の中のプロセスは見えないので、Windows の側の `wsl.exe` までを見る。
#[test]
fn wsl_closing_the_window_ends_the_child_processes_of_every_tab() {
    let Some(config) = wsl_config("wsl_closing_the_window") else {
        return;
    };
    let server = Server::start_watching_children(&config, &[]);
    exercise_closing_the_window(
        server,
        [SH_LONG_COMMAND, SH_LONG_COMMAND],
        &|_| {},
        wait_sh_ready,
    );
}
