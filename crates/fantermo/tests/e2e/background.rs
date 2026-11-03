use crate::common::CMD_COMMAND;
use crate::common::CMD_PROMPT;
use crate::common::Server;
use crate::ui::{
    BAND_BOTTOM, BAND_HEIGHT, BAND_TOP, BANDS_IMAGE, BANDS_SIDE, BackdropArea, DPI_100_PERCENT,
    DPI_150_PERCENT, DPI_200_PERCENT, MIDDLE_LEFT, MIDDLE_RIGHT, TERMINAL_BACKGROUND,
    TERMINAL_BACKGROUND_SETTING, bands_color, bands_image, change_dpi, number,
    over_terminal_background, scroll_bar_edge, solid_image, start_uneven_with_setup,
    start_with_background, wait_pixels,
};
use serde_json::Value;

/// `image_opacity = 1` で表示方法 `fit` の [`BANDS_IMAGE`] を背景にして起動する。
fn start_with_fit(fit: &str) -> (Server, BackdropArea) {
    let server = start_with_background(
        &format!(
            "[background]\nimage_opacity = 1.0\nfit = \"{fit}\"\n\n[[background.images]]\npath = '{BANDS_IMAGE}'\n"
        ),
        |_| {},
    );
    let state = server.state();
    assert_eq!(state["background"]["image"], BANDS_IMAGE, "{state:#}");
    let area = BackdropArea::of(&state);
    // 表示方法を見分ける位置は、横に長い範囲を前提にしている。
    assert!(
        f64::from(area.width) > 1.4 * f64::from(area.height),
        "{area:?}"
    );
    (server, area)
}

#[test]
fn a_stretched_background_image_shows_all_of_it_and_only_in_its_area() {
    let (server, area) = start_with_fit("stretch");
    wait_pixels(
        &server,
        &[
            (area.at(0.25, 0.03), BAND_TOP),
            (area.at(0.25, 0.5), MIDDLE_LEFT),
            (area.at(0.75, 0.5), MIDDLE_RIGHT),
            (area.at(0.75, 0.97), BAND_BOTTOM),
        ],
    );
    // スクロールバーの列にも敷き、タブバーには敷かない。
    wait_pixels(&server, &[(scroll_bar_edge(&server.state()), MIDDLE_RIGHT)]);
    let picture = server.picture();
    let (tab_x, tab_y) = area.at(0.75, 0.0);
    assert_ne!(picture.rgb(tab_x, tab_y - 1), BAND_TOP);
}

/// 範囲を覆うように広げ、はみ出した上下を切るので、上下の帯は見えない。
#[test]
fn a_filling_background_image_cuts_what_overflows() {
    let (server, area) = start_with_fit("fill");
    wait_pixels(
        &server,
        &[
            (area.at(0.25, 0.03), MIDDLE_LEFT),
            (area.at(0.25, 0.97), MIDDLE_LEFT),
            (area.at(0.75, 0.03), MIDDLE_RIGHT),
            (area.at(0.75, 0.97), MIDDLE_RIGHT),
        ],
    );
}

/// 全体が入る大きさで真ん中に置き、左右の余りは背景色になる。
#[test]
fn a_fitted_background_image_is_whole_in_the_middle() {
    let (server, area) = start_with_fit("fit");
    let side = area.height;
    let left = area.x + (area.width - side) / 2;
    let quarter = left + side / 4;
    let three_quarters = left + side * 3 / 4;
    let middle = area.y + side / 2;
    wait_pixels(
        &server,
        &[
            ((quarter, middle), MIDDLE_LEFT),
            ((three_quarters, middle), MIDDLE_RIGHT),
            ((quarter, area.y + side * 3 / 100), BAND_TOP),
            ((quarter, area.y + side * 97 / 100), BAND_BOTTOM),
            ((area.x + (left - area.x) / 2, middle), TERMINAL_BACKGROUND),
            (
                (area.x + area.width - (left - area.x) / 2, middle),
                TERMINAL_BACKGROUND,
            ),
        ],
    );
}

/// 元の大きさで真ん中に置き、周りは背景色になる。
#[test]
fn a_centered_background_image_keeps_its_size() {
    let (server, area) = start_with_fit("center");
    let (x, y) = area.at(0.5, 0.5);
    let side = BANDS_SIDE;
    wait_pixels(
        &server,
        &[
            ((x - side / 4, y), MIDDLE_LEFT),
            ((x + side / 4, y), MIDDLE_RIGHT),
            ((x - side / 4, y - side / 2 + BAND_HEIGHT / 2), BAND_TOP),
            ((x - side / 4, y + side / 2 - BAND_HEIGHT / 2), BAND_BOTTOM),
            ((x - side * 3 / 4, y), TERMINAL_BACKGROUND),
            ((x + side * 3 / 4, y), TERMINAL_BACKGROUND),
            ((x - side / 4, y - side * 3 / 4), TERMINAL_BACKGROUND),
            ((x - side / 4, y + side * 3 / 4), TERMINAL_BACKGROUND),
        ],
    );
}

/// 元の大きさで範囲の左上から敷き詰める。プロンプトの文字を避けて、右下へ離れたところを見る。
#[test]
fn a_tiled_background_image_repeats_from_the_top_left() {
    let (server, area) = start_with_fit("tile");
    let side = BANDS_SIDE;
    let (left, top) = (area.x + 2 * side, area.y + side);
    wait_pixels(
        &server,
        &[
            ((left + side / 4, top + side / 2), MIDDLE_LEFT),
            ((left + side * 3 / 4, top + side / 2), MIDDLE_RIGHT),
            ((left + side / 4, top + BAND_HEIGHT / 2), BAND_TOP),
            ((left + side / 4, top + side - BAND_HEIGHT / 2), BAND_BOTTOM),
            ((left + side + side / 4, top + side / 2), MIDDLE_LEFT),
            ((left + side / 4, top + side + side / 2), MIDDLE_LEFT),
        ],
    );
}

/// 範囲の左上から敷き詰めた [`BANDS_IMAGE`] の、右端と下端に丸ごとは入らない最後の 1 枚の画素と、不透明度
/// `image_opacity` で見える色。その 1 枚の最初の列・真ん中・範囲の最後の列（行も同じ）を見る。右端はプロンプトの
/// 文字を避けて、2 枚目の行から下を見る。
fn tiled_edge_pixels(area: BackdropArea, image_opacity: f64) -> Vec<((u32, u32), [u8; 3])> {
    let side = BANDS_SIDE;
    assert!(
        area.width > 2 * side && area.height > 2 * side,
        "the area is too small: {area:?}"
    );
    let partial_columns = partial_tile(area.width);
    let partial_rows = partial_tile(area.height);
    let mut columns_beside = vec![side + side / 4, side + side * 3 / 4];
    columns_beside.extend(&partial_columns);
    let mut rows_beside = vec![side + BAND_HEIGHT / 2, side + side / 2];
    rows_beside.extend(&partial_rows);

    let pixel = |x: u32, y: u32| {
        let color = bands_color(x % side, y % side);
        (
            (area.x + x, area.y + y),
            over_terminal_background(color, image_opacity),
        )
    };
    let mut pixels = Vec::new();
    for &x in &partial_columns {
        pixels.extend(rows_beside.iter().map(|&y| pixel(x, y)));
    }
    for &y in &partial_rows {
        pixels.extend(columns_beside.iter().map(|&x| pixel(x, y)));
    }
    pixels
}

/// 範囲の左上から敷き詰めた [`BANDS_IMAGE`] の、スクロールバーの列の最初と最後の列の画素と、不透明度
/// `image_opacity` で見える色。敷き詰めはスクロールバーの列まで続く。つまみがないときに見る（つまみは列の真ん中に
/// 描く）。
fn tiled_scroll_bar_pixels(state: &Value, image_opacity: f64) -> Vec<((u32, u32), [u8; 3])> {
    let area = BackdropArea::of(state);
    assert!(state["scroll_bar"]["thumb"].is_null(), "{state:#}");
    let bar_x = u32::try_from(number(&state["scroll_bar"]["x"])).expect("the position fits");
    let right = area.x + area.width;
    assert_eq!(
        u64::from(right),
        number(&state["client_width"]),
        "{state:#}"
    );
    assert!(area.x < bar_x && bar_x < right, "{state:#}");
    let side = BANDS_SIDE;
    let rows = [side + BAND_HEIGHT / 2, side + side / 2, area.height - 1];
    let mut pixels = Vec::new();
    for x in [bar_x, right - 1] {
        for &y in &rows {
            let color = bands_color((x - area.x) % side, y % side);
            pixels.push((
                (x, area.y + y),
                over_terminal_background(color, image_opacity),
            ));
        }
    }
    pixels
}

/// 長さ `length` の範囲に [`BANDS_IMAGE`] を敷き詰めたときの、丸ごとは入らない最後の 1 枚の最初・真ん中・最後の位置。
/// 割り切れれば空。
fn partial_tile(length: u32) -> Vec<u32> {
    let start = length / BANDS_SIDE * BANDS_SIDE;
    if start == length {
        return Vec::new();
    }
    vec![start, start + (length - start - 1) / 2, length - 1]
}

/// 背景画像の設定 `background` で、セルにも [`BANDS_IMAGE`] にも割り切れない大きさのウィンドウを開き、シェルの
/// プロンプトまで待つ。
fn start_uneven_with_background(background: &str) -> (Server, BackdropArea) {
    let (server, state) = start_uneven_with_setup(
        &format!("{TERMINAL_BACKGROUND_SETTING}{background}"),
        &CMD_COMMAND,
        CMD_PROMPT,
        |dir| {
            std::fs::write(dir.join(BANDS_IMAGE), bands_image())
                .expect("failed to write the image");
        },
    );
    assert_eq!(state["background"]["image"], BANDS_IMAGE, "{state:#}");
    let area = BackdropArea::of(&state);
    assert!(
        !area.width.is_multiple_of(BANDS_SIDE) && !area.height.is_multiple_of(BANDS_SIDE),
        "the area fits the image: {area:?}"
    );
    (server, area)
}

/// 敷き詰めは範囲の右端と下端まで描き、丸ごとは入らない最後の 1 枚もはみ出す分を切って描く。右端はスクロールバーの
/// 列の右端。DPI を変えたあとの新しい範囲でも同じ。
#[test]
fn a_tiled_background_image_reaches_the_right_and_bottom_edges() {
    let (server, area) = start_uneven_with_background(&format!(
        "[background]\nimage_opacity = 1.0\nfit = \"tile\"\n\n[[background.images]]\npath = '{BANDS_IMAGE}'\n"
    ));
    let mut expected = tiled_edge_pixels(area, 1.0);
    expected.extend(tiled_scroll_bar_pixels(&server.state(), 1.0));
    wait_pixels(&server, &expected);

    for dpi in [DPI_150_PERCENT, DPI_200_PERCENT, DPI_100_PERCENT] {
        let state = change_dpi(&server, dpi);
        assert_eq!(state["background"]["image"], BANDS_IMAGE, "{state:#}");
        let mut expected = tiled_edge_pixels(BackdropArea::of(&state), 1.0);
        expected.extend(tiled_scroll_bar_pixels(&state, 1.0));
        wait_pixels(&server, &expected);
    }
}

/// 項目に書いた敷き詰めと不透明度でも、右端と下端の最後の 1 枚を描く。
#[test]
fn a_tiled_image_entry_reaches_the_right_and_bottom_edges_with_its_opacity() {
    let (server, area) = start_uneven_with_background(&format!(
        "[background]\nimage_opacity = 1.0\nfit = \"stretch\"\n\n[[background.images]]\npath = '{BANDS_IMAGE}'\nfit = \"tile\"\nimage_opacity = 0.5\n"
    ));
    let mut expected = tiled_edge_pixels(area, 0.5);
    expected.extend(tiled_scroll_bar_pixels(&server.state(), 0.5));
    wait_pixels(&server, &expected);
}

#[test]
fn the_opacity_of_the_background_image_sets_how_much_of_it_shows() {
    let settings = [
        ("image_opacity = 0.0\n", 0.0),
        ("image_opacity = 0.5\n", 0.5),
        ("image_opacity = 1.0\n", 1.0),
        // 既定の値。
        ("", 0.2),
    ];
    for (opacity, expected) in settings {
        let server = start_with_background(
            &format!(
                "[background]\nfit = \"stretch\"\n{opacity}\n[[background.images]]\npath = '{BANDS_IMAGE}'\n"
            ),
            |_| {},
        );
        let area = BackdropArea::of(&server.state());
        wait_pixels(
            &server,
            &[
                (
                    area.at(0.25, 0.5),
                    over_terminal_background(MIDDLE_LEFT, expected),
                ),
                (
                    area.at(0.75, 0.5),
                    over_terminal_background(MIDDLE_RIGHT, expected),
                ),
            ],
        );
    }
}

/// 項目に書いた表示方法と不透明度は `[background]` の値より優先し、書かなかったものは `[background]` の値に従う。
#[test]
fn an_image_entry_overrides_the_fit_and_the_opacity_it_writes() {
    const WHOLE: &str = "[background]\nfit = \"stretch\"\nimage_opacity = 1.0\n\n";
    let image = format!("[[background.images]]\npath = '{BANDS_IMAGE}'\n");

    // どちらも項目の値: 敷き詰めて半透明。
    let server = start_with_background(
        &format!("{WHOLE}{image}fit = \"tile\"\nimage_opacity = 0.5\n"),
        |_| {},
    );
    let area = BackdropArea::of(&server.state());
    let (left, top) = (area.x + 2 * BANDS_SIDE, area.y + BANDS_SIDE);
    let half_left = over_terminal_background(MIDDLE_LEFT, 0.5);
    wait_pixels(
        &server,
        &[
            ((left + BANDS_SIDE / 4, top + BANDS_SIDE / 2), half_left),
            (
                (left + BANDS_SIDE + BANDS_SIDE / 4, top + BANDS_SIDE / 2),
                half_left,
            ),
        ],
    );
    drop(server);

    // 不透明度だけ項目の値: 全体の表示方法で半透明。
    let server = start_with_background(&format!("{WHOLE}{image}image_opacity = 0.5\n"), |_| {});
    let area = BackdropArea::of(&server.state());
    wait_pixels(
        &server,
        &[
            (area.at(0.25, 0.03), over_terminal_background(BAND_TOP, 0.5)),
            (area.at(0.25, 0.5), half_left),
        ],
    );
    drop(server);

    // 表示方法だけ項目の値: 全体の不透明度で敷き詰める。
    let server = start_with_background(&format!("{WHOLE}{image}fit = \"tile\"\n"), |_| {});
    let area = BackdropArea::of(&server.state());
    let (left, top) = (area.x + 2 * BANDS_SIDE, area.y + BANDS_SIDE);
    wait_pixels(
        &server,
        &[
            ((left + BANDS_SIDE / 4, top + BANDS_SIDE / 2), MIDDLE_LEFT),
            (
                (left + BANDS_SIDE + BANDS_SIDE / 4, top + BANDS_SIDE / 2),
                MIDDLE_LEFT,
            ),
        ],
    );
}

#[test]
fn the_background_image_follows_the_settings_file() {
    let server = start_with_background("", |_| {});
    let state = server.state();
    assert_eq!(state["background"]["image"], Value::Null, "{state:#}");
    let area = BackdropArea::of(&state);
    wait_pixels(&server, &[(area.at(0.25, 0.5), TERMINAL_BACKGROUND)]);

    let write = |background: String| {
        std::fs::write(
            server.dir().join("config.toml"),
            format!("{TERMINAL_BACKGROUND_SETTING}{background}"),
        )
        .expect("failed to write the settings");
    };
    write(format!(
        "[background]\nfit = \"stretch\"\nimage_opacity = 1.0\n\n[[background.images]]\npath = '{BANDS_IMAGE}'\n"
    ));
    wait_pixels(&server, &[(area.at(0.25, 0.5), MIDDLE_LEFT)]);

    write(format!(
        "[background]\nfit = \"fill\"\nimage_opacity = 0.5\n\n[[background.images]]\npath = '{BANDS_IMAGE}'\n"
    ));
    wait_pixels(
        &server,
        &[(
            area.at(0.25, 0.03),
            over_terminal_background(MIDDLE_LEFT, 0.5),
        )],
    );

    write(String::new());
    wait_pixels(&server, &[(area.at(0.25, 0.5), TERMINAL_BACKGROUND)]);
}

/// 読めない画像は外して別の候補を選び、全部読めなければ画像なしで動く。
#[test]
fn unreadable_background_images_are_skipped() {
    const BROKEN_IMAGE: &str = "broken.png";
    let write_broken = |dir: &std::path::Path| {
        std::fs::write(dir.join(BROKEN_IMAGE), b"not an image").expect("failed to write");
    };
    let entries = |paths: &[&str]| {
        let tables: Vec<String> = paths
            .iter()
            .map(|path| format!("[[background.images]]\npath = '{path}'\n"))
            .collect();
        format!(
            "[background]\nfit = \"stretch\"\nimage_opacity = 1.0\n\n{}",
            tables.join("")
        )
    };

    let server = start_with_background(
        &entries(&[BROKEN_IMAGE, "no-such-file.png", BANDS_IMAGE]),
        write_broken,
    );
    let state = server.state();
    assert_eq!(state["background"]["image"], BANDS_IMAGE, "{state:#}");
    let area = BackdropArea::of(&state);
    wait_pixels(&server, &[(area.at(0.25, 0.5), MIDDLE_LEFT)]);
    drop(server);

    let server = start_with_background(&entries(&[BROKEN_IMAGE, "no-such-file.png"]), write_broken);
    let state = server.state();
    assert_eq!(state["background"]["image"], Value::Null, "{state:#}");
    let area = BackdropArea::of(&state);
    wait_pixels(&server, &[(area.at(0.25, 0.5), TERMINAL_BACKGROUND)]);
}

/// タブごとにフォルダの中から選び、切り替えても選んだ画像のまま。選ぶのはランダムなので、タブの選んだ名前と絵を
/// 比べる。
#[test]
fn each_tab_keeps_the_background_image_it_chose() {
    const IMAGES: [(&str, [u8; 3]); 2] = [("red.png", [255, 0, 0]), ("blue.png", [0, 0, 255])];
    const TABS: usize = 4;
    let color_of = |state: &Value| {
        IMAGES
            .iter()
            .find(|(name, _)| state["background"]["image"] == *name)
            .unwrap_or_else(|| panic!("an unknown image: {state:#}"))
            .1
    };
    let server = start_with_background(
        "[background]\nfit = \"stretch\"\nimage_opacity = 1.0\n\n[[background.images]]\npath = 'images'\n",
        |dir| {
            let images = dir.join("images");
            std::fs::create_dir(&images).expect("failed to create the folder");
            for (name, color) in IMAGES {
                std::fs::write(images.join(name), solid_image(color))
                    .expect("failed to write the image");
            }
            // 拡張子が画像でないファイルは候補にしない。
            std::fs::write(images.join("notes.txt"), b"x").expect("failed to write");
        },
    );
    for _ in 2..=TABS {
        server.open_tab(CMD_PROMPT);
    }
    for tab in 0..TABS {
        server.post(&format!("/e2e/tab/select/{tab}"), b"");
        let first = server.wait_until("the tab to be shown", |state| state["active"] == tab);
        let area = BackdropArea::of(&first);
        wait_pixels(&server, &[(area.at(0.25, 0.5), color_of(&first))]);
        server.post(&format!("/e2e/tab/select/{}", (tab + 1) % TABS), b"");
        server.post(&format!("/e2e/tab/select/{tab}"), b"");
        let again = server.wait_until("the tab to be shown", |state| state["active"] == tab);
        assert_eq!(again["background"]["image"], first["background"]["image"]);
        wait_pixels(&server, &[(area.at(0.25, 0.5), color_of(&again))]);
    }
}
