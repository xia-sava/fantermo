//! 設定ダイアログのコントロールを作る部品（[`Builder`]）と、種類ごとのコントロールの値を出し入れする
//! 部品（一覧、数の入力欄、ラジオボタンの組、固定の選択肢のコンボボックス）。

use std::ops::RangeInclusive;

use fantermo_core::config::Config;
use windows::{
    Win32::{
        Foundation::{HINSTANCE, HWND, LPARAM, WPARAM},
        UI::{
            Controls::{
                BST_CHECKED, UDM_SETBUDDY, UDM_SETPOS32, UDM_SETRANGE32, UDS_ALIGNRIGHT,
                UDS_ARROWKEYS, UDS_HOTTRACK, UDS_NOTHOUSANDS, UPDOWN_CLASSW, WC_COMBOBOXW,
                WC_LISTBOXW,
            },
            Input::KeyboardAndMouse::{EnableWindow, IsWindowEnabled},
            WindowsAndMessaging::{
                BM_GETCHECK, BM_SETCHECK, BS_AUTORADIOBUTTON, BS_DEFPUSHBUTTON, BS_GROUPBOX,
                BS_PUSHBUTTON, CB_ADDSTRING, CB_DELETESTRING, CB_GETCURSEL, CB_INSERTSTRING,
                CB_SETCURSEL, CBS_DROPDOWNLIST, CreateWindowExW, ES_AUTOHSCROLL,
                GetWindowTextLengthW, GetWindowTextW, HMENU, LB_ADDSTRING, LB_DELETESTRING,
                LB_GETCURSEL, LB_INSERTSTRING, LB_RESETCONTENT, LB_SETCURSEL, LB_SETTABSTOPS,
                LBS_NOTIFY, LBS_USETABSTOPS, SendMessageW, SetWindowTextW, WINDOW_EX_STYLE,
                WINDOW_STYLE, WS_CHILD, WS_EX_CLIENTEDGE, WS_GROUP, WS_TABSTOP, WS_VISIBLE,
                WS_VSCROLL,
            },
        },
    },
    core::{HSTRING, PCWSTR, w},
};

/// 見出しや説明の文字。押せないので同じ ID でよい。
const ID_LABEL: i32 = -1;

/// 上下の矢印のボタンの範囲（下限、上限）。上の矢印で増える向きにするためのもの。
const SPIN_RANGE: (i32, i32) = (0, 1000);

/// 96 DPI のときの矩形（左、上、幅、高さ）。
pub(super) type DipRect = [i32; 4];

/// ダイアログの子コントロールを作る。矩形を渡したコントロールは [`Builder::placed`] に控え、
/// あとで [`super::State::layout`] が DPI に合わせて置く。
pub(super) struct Builder {
    /// 親のダイアログの HWND。ダイアログが閉じるまで有効で、作るコントロールはその子になる。
    hwnd: HWND,
    /// コントロールを作るモジュール（この実行ファイル）。
    instance: HINSTANCE,
    pub(super) placed: Vec<(HWND, DipRect)>,
}

impl Builder {
    /// ダイアログ `hwnd` の子コントロールを、モジュール `instance` で作るもの。
    pub(super) fn new(hwnd: HWND, instance: HINSTANCE) -> Self {
        Self {
            hwnd,
            instance,
            placed: Vec::new(),
        }
    }

    /// `class` の子コントロールを作る。`rect` が `None` のものは置かない。
    fn child(
        &mut self,
        class: PCWSTR,
        text: &str,
        style: WINDOW_STYLE,
        ex_style: WINDOW_EX_STYLE,
        id: i32,
        rect: Option<DipRect>,
    ) -> windows::core::Result<HWND> {
        // SAFETY: hwnd は有効なウィンドウで、文字列はこの呼び出しの間は生きている。
        let control = unsafe {
            CreateWindowExW(
                ex_style,
                class,
                &HSTRING::from(text),
                WS_CHILD | WS_VISIBLE | style,
                0,
                0,
                0,
                0,
                Some(self.hwnd),
                Some(HMENU(id as isize as *mut _)),
                Some(self.instance),
                None,
            )?
        };
        if let Some(rect) = rect {
            self.placed.push((control, rect));
        }
        Ok(control)
    }

    /// 見出しや説明の文字。
    pub(super) fn label(&mut self, text: &str, rect: DipRect) -> windows::core::Result<HWND> {
        self.child(
            w!("STATIC"),
            text,
            WINDOW_STYLE(0),
            WINDOW_EX_STYLE(0),
            ID_LABEL,
            Some(rect),
        )
    }

    /// 文字をあとから入れる、ID つきの文字（値の表示、警告、エラー、色の見本）。
    pub(super) fn static_text(
        &mut self,
        id: i32,
        ex_style: WINDOW_EX_STYLE,
        rect: DipRect,
    ) -> windows::core::Result<HWND> {
        self.child(w!("STATIC"), "", WINDOW_STYLE(0), ex_style, id, Some(rect))
    }

    /// 枠線つきのグループ。
    pub(super) fn group(&mut self, text: &str, rect: DipRect) -> windows::core::Result<HWND> {
        self.child(
            w!("BUTTON"),
            text,
            WINDOW_STYLE(BS_GROUPBOX as u32),
            WINDOW_EX_STYLE(0),
            ID_LABEL,
            Some(rect),
        )
    }

    /// 1 行の入力欄。
    pub(super) fn edit(&mut self, id: i32, rect: DipRect) -> windows::core::Result<HWND> {
        self.child(
            w!("EDIT"),
            "",
            WS_GROUP | WS_TABSTOP | WINDOW_STYLE(ES_AUTOHSCROLL as u32),
            WS_EX_CLIENTEDGE,
            id,
            Some(rect),
        )
    }

    /// 入力欄の上下の矢印。置く位置は入力欄に付けて決める。
    fn spin(&mut self, id: i32) -> windows::core::Result<HWND> {
        self.child(
            UPDOWN_CLASSW,
            "",
            WINDOW_STYLE(UDS_ALIGNRIGHT | UDS_ARROWKEYS | UDS_NOTHOUSANDS | UDS_HOTTRACK),
            WINDOW_EX_STYLE(0),
            id,
            None,
        )
    }

    /// 押しボタン。
    pub(super) fn button(
        &mut self,
        id: i32,
        text: &str,
        rect: DipRect,
    ) -> windows::core::Result<HWND> {
        self.push_button(id, text, rect, BS_PUSHBUTTON)
    }

    /// Enter で押される既定の押しボタン。
    pub(super) fn default_button(
        &mut self,
        id: i32,
        text: &str,
        rect: DipRect,
    ) -> windows::core::Result<HWND> {
        self.push_button(id, text, rect, BS_DEFPUSHBUTTON)
    }

    /// ボタンのスタイル `style`（`BS_PUSHBUTTON` か `BS_DEFPUSHBUTTON`）の押しボタン。
    fn push_button(
        &mut self,
        id: i32,
        text: &str,
        rect: DipRect,
        style: i32,
    ) -> windows::core::Result<HWND> {
        self.child(
            w!("BUTTON"),
            text,
            WS_GROUP | WS_TABSTOP | WINDOW_STYLE(style as u32),
            WINDOW_EX_STYLE(0),
            id,
            Some(rect),
        )
    }

    /// ラジオボタン。組の先頭（`first`）だけが Tab で止まり、グループを始める。
    fn radio(
        &mut self,
        id: i32,
        text: &str,
        rect: DipRect,
        first: bool,
    ) -> windows::core::Result<HWND> {
        let group = if first {
            WS_GROUP | WS_TABSTOP
        } else {
            WINDOW_STYLE(0)
        };
        self.child(
            w!("BUTTON"),
            text,
            group | WINDOW_STYLE(BS_AUTORADIOBUTTON as u32),
            WINDOW_EX_STYLE(0),
            id,
            Some(rect),
        )
    }

    /// ドロップダウンの選択肢のコンボボックス。選択肢は `labels` の順に入れる。
    pub(super) fn combo<S: AsRef<str>>(
        &mut self,
        id: i32,
        rect: DipRect,
        labels: impl IntoIterator<Item = S>,
    ) -> windows::core::Result<HWND> {
        let combo = self.child(
            WC_COMBOBOXW,
            "",
            WS_GROUP | WS_TABSTOP | WS_VSCROLL | WINDOW_STYLE(CBS_DROPDOWNLIST as u32),
            WINDOW_EX_STYLE(0),
            id,
            Some(rect),
        )?;
        for text in labels {
            // SAFETY: combo は有効なコンボボックスで、文字列はこの呼び出しの間は生きている。
            unsafe {
                SendMessageW(
                    combo,
                    CB_ADDSTRING,
                    None,
                    Some(LPARAM(HSTRING::from(text.as_ref()).as_ptr() as isize)),
                )
            };
        }
        Ok(combo)
    }

    /// 一覧。`tab_stops`（ダイアログの単位）があれば、行の文字をタブで区切って桁を揃える。
    pub(super) fn list(
        &mut self,
        id: i32,
        rect: DipRect,
        tab_stops: &[i32],
    ) -> windows::core::Result<ListBox> {
        let mut style = LBS_NOTIFY;
        if !tab_stops.is_empty() {
            style |= LBS_USETABSTOPS;
        }
        let list = ListBox(self.child(
            WC_LISTBOXW,
            "",
            WS_GROUP | WS_TABSTOP | WS_VSCROLL | WINDOW_STYLE(style as u32),
            WS_EX_CLIENTEDGE,
            id,
            Some(rect),
        )?);
        if !tab_stops.is_empty() {
            list.set_tab_stops(tab_stops);
        }
        Ok(list)
    }
}

/// 一覧（リストボックス）の操作。ダイアログの子の、有効なリストボックスの HWND を包む（ダイアログが
/// 閉じるまで有効）。
#[derive(Clone, Copy)]
pub(super) struct ListBox(HWND);

impl ListBox {
    /// 一覧の HWND。
    pub(super) fn hwnd(self) -> HWND {
        self.0
    }

    /// 行を末尾に足す。
    pub(super) fn add(self, text: &str) {
        // SAFETY: 一覧は有効なウィンドウで、文字列はこの呼び出しの間は生きている。
        unsafe {
            SendMessageW(
                self.0,
                LB_ADDSTRING,
                None,
                Some(LPARAM(HSTRING::from(text).as_ptr() as isize)),
            )
        };
    }

    /// 行を `rows` に入れ替える。選択は外れる。
    pub(super) fn reset_with<S: AsRef<str>>(self, rows: impl IntoIterator<Item = S>) {
        // SAFETY: 一覧は有効なウィンドウ。
        unsafe { SendMessageW(self.0, LB_RESETCONTENT, None, None) };
        for row in rows {
            self.add(row.as_ref());
        }
    }

    /// `index` 番目の行を選ぶ。
    pub(super) fn select(self, index: usize) {
        // SAFETY: 一覧は有効なウィンドウ。
        unsafe { SendMessageW(self.0, LB_SETCURSEL, Some(WPARAM(index)), None) };
    }

    /// 選んでいる行の位置。選んでいなければ `None`。
    pub(super) fn selected(self) -> Option<usize> {
        // SAFETY: 一覧は有効なウィンドウ。
        let index = unsafe { SendMessageW(self.0, LB_GETCURSEL, None, None) }.0;
        usize::try_from(index).ok()
    }

    /// `index` 番目の行の文字を書き直して、その行を選ぶ。
    pub(super) fn replace_row(self, index: usize, text: &str) {
        // SAFETY: 一覧は有効なウィンドウで、文字列はこの呼び出しの間は生きている。
        unsafe {
            SendMessageW(self.0, LB_DELETESTRING, Some(WPARAM(index)), None);
            SendMessageW(
                self.0,
                LB_INSERTSTRING,
                Some(WPARAM(index)),
                Some(LPARAM(HSTRING::from(text).as_ptr() as isize)),
            );
        }
        self.select(index);
    }

    fn set_tab_stops(self, tab_stops: &[i32]) {
        // SAFETY: 一覧は有効なウィンドウで、tab_stops はこの呼び出しの間は生きている。
        unsafe {
            SendMessageW(
                self.0,
                LB_SETTABSTOPS,
                Some(WPARAM(tab_stops.len())),
                Some(LPARAM(tab_stops.as_ptr() as isize)),
            )
        };
    }
}

/// 数の入力欄の値の扱い。
pub(super) struct NumberKind<T> {
    /// 入力欄の文字を値にする。読めなければ `None`。
    pub(super) parse: fn(&str) -> Option<T>,
    /// 値を入力欄に見せる文字にする。
    pub(super) format: fn(T) -> String,
    /// 値を矢印で一つ動かす（二つ目の引数の符号の向きへ）。範囲の端で止める。
    pub(super) step: fn(T, i32) -> T,
    /// 入力欄が読めないときに矢印で動かし始める値を、設定から取る。
    pub(super) start: fn(&Config) -> T,
    /// 入力欄が読めないときに見せる文。
    pub(super) message: &'static str,
}

/// スクロールバーの幅、余白、貼り付けの確認の大きさを、矢印で一つ動かした値。範囲 `range` の端で止める。
pub(super) fn step_integer(current: u32, delta: i32, range: &RangeInclusive<u32>) -> u32 {
    let next = if delta > 0 {
        current.saturating_add(1)
    } else {
        current.saturating_sub(1)
    };
    next.clamp(*range.start(), *range.end())
}

/// 入力欄に見せる、整数の値。
pub(super) fn format_integer(value: u32) -> String {
    value.to_string()
}

/// 数の入力欄と、その右に付ける上下の矢印。
pub(super) struct NumberField<T> {
    /// 入力欄の ID。
    id: i32,
    /// 入力欄。
    edit: HWND,
    /// 入力欄の右に付ける上下の矢印。
    spin: HWND,
    kind: NumberKind<T>,
}

impl<T: Copy> NumberField<T> {
    /// 入力欄 `id` を `rect` に作り、続けて上下の矢印 `spin_id` を作る。
    pub(super) fn create(
        b: &mut Builder,
        id: i32,
        spin_id: i32,
        rect: DipRect,
        kind: NumberKind<T>,
    ) -> windows::core::Result<Self> {
        let edit = b.edit(id, rect)?;
        let spin = b.spin(spin_id)?;
        Ok(Self {
            id,
            edit,
            spin,
            kind,
        })
    }

    /// 入力欄と上下の矢印。
    pub(super) fn controls(&self) -> [HWND; 2] {
        [self.edit, self.spin]
    }

    /// 入力欄に `value` を入れる。
    pub(super) fn fill(&self, value: T) {
        set_text(self.edit, &(self.kind.format)(value));
    }

    /// 入力欄を空にする。
    pub(super) fn clear(&self) {
        set_text(self.edit, "");
    }

    /// 入力欄を `value` にする（`None` なら空にする）。いまの文字と同じなら書かない（書くと `EN_CHANGE` が
    /// 届く）。
    pub(super) fn fill_if_changed(&self, value: Option<T>) {
        let text = value.map(self.kind.format).unwrap_or_default();
        if get_text(self.edit) != text {
            set_text(self.edit, &text);
        }
    }

    /// 入力欄の値。読めなければ、見せる文と入力欄。
    pub(super) fn read(&self) -> Result<T, (&'static str, HWND)> {
        (self.kind.parse)(&get_text(self.edit)).ok_or((self.kind.message, self.edit))
    }
}

/// 値の型の違う数の入力欄を、まとめて並べ、知らせを振り分けるための操作。
pub(super) trait NumberInput {
    /// ID が `id` のコントロールがこの入力欄か。
    fn owns(&self, id: i32) -> bool;
    /// 上下の矢印。
    fn spin(&self) -> HWND;
    /// 上下の矢印を入力欄の右端に付け直す（付けるときに入力欄の幅を詰める）。
    fn attach_spin(&self);
    /// 入力欄の値を `delta` の向きに一つ動かす。読めなければ `start` の値から動かす。
    fn step(&self, delta: i32, start: &Config);
}

impl<T: Copy> NumberInput for NumberField<T> {
    fn owns(&self, id: i32) -> bool {
        id == self.id
    }

    fn spin(&self) -> HWND {
        self.spin
    }

    fn attach_spin(&self) {
        // 範囲は、上の矢印で数が増える向き（既定は逆）にするためだけに決める。数は step で決める。
        // SAFETY: 矢印と入力欄はダイアログの子の有効なウィンドウ。
        unsafe {
            SendMessageW(
                self.spin,
                UDM_SETBUDDY,
                Some(WPARAM(self.edit.0 as usize)),
                None,
            );
            SendMessageW(
                self.spin,
                UDM_SETRANGE32,
                Some(WPARAM(SPIN_RANGE.0 as usize)),
                Some(LPARAM(SPIN_RANGE.1 as isize)),
            );
            // 端にいると矢印を押しても知らせないので、真ん中に置く（step で動かさない）。
            SendMessageW(
                self.spin,
                UDM_SETPOS32,
                None,
                Some(LPARAM(((SPIN_RANGE.0 + SPIN_RANGE.1) / 2) as isize)),
            );
            // 付け直すと矢印が触れるようになるので、相手の入力欄に合わせる。
            _ = EnableWindow(self.spin, IsWindowEnabled(self.edit).as_bool());
        }
    }

    fn step(&self, delta: i32, start: &Config) {
        let current = self.read().unwrap_or_else(|_| (self.kind.start)(start));
        self.fill((self.kind.step)(current, delta));
    }
}

/// ラジオボタンの組。選択肢の順にボタンが並び、ID も続く。
pub(super) struct RadioGroup<T: 'static> {
    /// 先頭のボタンの ID。
    first_id: i32,
    /// ボタン。`choices` の順。
    buttons: Vec<HWND>,
    choices: &'static [T],
}

impl<T: Copy + PartialEq> RadioGroup<T> {
    /// 選択肢 `choices` のボタンを、それぞれの表示 `labels` で、選択肢の位置 `rect_of(位置)` に作る。ID は
    /// `first_id` から続く。先頭のボタンだけが Tab で止まり、グループを始める。
    pub(super) fn create<const N: usize>(
        b: &mut Builder,
        first_id: i32,
        choices: &'static [T; N],
        labels: &[&str; N],
        rect_of: impl Fn(i32) -> DipRect,
    ) -> windows::core::Result<Self> {
        let mut buttons = Vec::with_capacity(N);
        for (index, text) in (0..).zip(labels) {
            buttons.push(b.radio(first_id + index, text, rect_of(index), index == 0)?);
        }
        Ok(Self {
            first_id,
            buttons,
            choices,
        })
    }

    /// ボタン。選択肢の順。
    pub(super) fn buttons(&self) -> &[HWND] {
        &self.buttons
    }

    /// 選択肢 `value` のボタン。
    pub(super) fn button(&self, value: T) -> Option<HWND> {
        let index = self.choices.iter().position(|&choice| choice == value)?;
        self.buttons.get(index).copied()
    }

    /// `value` のボタンだけを選ぶ。
    pub(super) fn fill(&self, value: T) {
        self.check(self.choices.iter().position(|&choice| choice == value));
    }

    /// どのボタンも選ばない。
    pub(super) fn clear(&self) {
        self.check(None);
    }

    /// `index` 番目のボタンだけを選ぶ。`None` ならどれも選ばない。
    fn check(&self, index: Option<usize>) {
        for (i, &button) in self.buttons.iter().enumerate() {
            let state = if Some(i) == index { BST_CHECKED.0 } else { 0 };
            // SAFETY: button はダイアログの子の有効なボタン。
            unsafe { SendMessageW(button, BM_SETCHECK, Some(WPARAM(state as usize)), None) };
        }
    }

    /// 選んでいるボタンの選択肢。どれも選んでいなければ `None`。
    pub(super) fn read(&self) -> Option<T> {
        let index = self.buttons.iter().position(|&button| {
            // SAFETY: button はダイアログの子の有効なボタン。
            unsafe { SendMessageW(button, BM_GETCHECK, None, None) }.0 == BST_CHECKED.0 as isize
        })?;
        self.choices.get(index).copied()
    }
}

impl<T> RadioGroup<T> {
    /// ID が `id` のコントロールが、この組のボタンか。
    pub(super) fn owns(&self, id: i32) -> bool {
        usize::try_from(id - self.first_id).is_ok_and(|index| index < self.buttons.len())
    }
}

/// `CB_SETCURSEL` の位置で、どれも選ばないこと（-1）。
const CB_NO_SELECTION: usize = usize::MAX;

/// 固定の選択肢のコンボボックス。選択肢の順に表示が並ぶ。
pub(super) struct ChoiceBox<T: 'static> {
    combo: HWND,
    choices: &'static [T],
}

impl<T: Copy + PartialEq> ChoiceBox<T> {
    /// 選択肢 `choices` のコンボボックス `id` を、それぞれの表示 `labels` で `rect` に作る。
    pub(super) fn create<S: AsRef<str>>(
        b: &mut Builder,
        id: i32,
        rect: DipRect,
        choices: &'static [T],
        labels: impl IntoIterator<Item = S>,
    ) -> windows::core::Result<Self> {
        let combo = b.combo(id, rect, labels)?;
        Ok(Self { combo, choices })
    }

    /// コンボボックスの HWND。
    pub(super) fn hwnd(&self) -> HWND {
        self.combo
    }

    /// `value` を選ぶ。選択肢になければ何もしない。
    pub(super) fn fill(&self, value: T) {
        let Some(index) = self.choices.iter().position(|&choice| choice == value) else {
            return;
        };
        // SAFETY: コンボボックスはダイアログの子の有効なウィンドウ。
        unsafe { SendMessageW(self.combo, CB_SETCURSEL, Some(WPARAM(index)), None) };
    }

    /// どれも選ばない。
    pub(super) fn clear(&self) {
        // SAFETY: コンボボックスはダイアログの子の有効なウィンドウ。
        unsafe {
            SendMessageW(
                self.combo,
                CB_SETCURSEL,
                Some(WPARAM(CB_NO_SELECTION)),
                None,
            )
        };
    }

    /// 選んでいる選択肢。選んでいなければ `None`。
    pub(super) fn read(&self) -> Option<T> {
        // SAFETY: コンボボックスはダイアログの子の有効なウィンドウ。
        let index = unsafe { SendMessageW(self.combo, CB_GETCURSEL, None, None) }.0;
        usize::try_from(index)
            .ok()
            .and_then(|index| self.choices.get(index).copied())
    }

    /// 選択肢 `value` の表示を `text` に書き直す。選んでいるものは変えない。
    pub(super) fn relabel(&self, value: T, text: &str) {
        let Some(index) = self.choices.iter().position(|&choice| choice == value) else {
            return;
        };
        // SAFETY: コンボボックスはダイアログの子の有効なウィンドウで、文字列はこの呼び出しの間は生きている。
        unsafe {
            let selected = SendMessageW(self.combo, CB_GETCURSEL, None, None).0;
            SendMessageW(self.combo, CB_DELETESTRING, Some(WPARAM(index)), None);
            SendMessageW(
                self.combo,
                CB_INSERTSTRING,
                Some(WPARAM(index)),
                Some(LPARAM(HSTRING::from(text).as_ptr() as isize)),
            );
            SendMessageW(
                self.combo,
                CB_SETCURSEL,
                Some(WPARAM(selected as usize)),
                None,
            );
        }
    }
}

pub(super) fn set_text(control: HWND, text: &str) {
    // SAFETY: control は有効なウィンドウで、文字列はこの呼び出しの間は生きている。
    unsafe { _ = SetWindowTextW(control, &HSTRING::from(text)) };
}

pub(super) fn get_text(control: HWND) -> String {
    // SAFETY: control は有効なウィンドウ。
    let length = unsafe { GetWindowTextLengthW(control) }.max(0) as usize;
    let mut buffer = vec![0u16; length + 1];
    // SAFETY: buffer は length + 1 個の u16 を書き込める。
    let copied = unsafe { GetWindowTextW(control, &mut buffer) }.max(0) as usize;
    String::from_utf16_lossy(&buffer[..copied])
}
