//! タブバーの色（[`Palette`]）と、描くもの（[`Paint`]）の組み立て。

use fantermo_core::{config::TAB_BAR_COLOR_KEYS, frame::Rgb};

use super::{
    BAR_TINT, BELL_COLOR, HOVERED_BUTTON_TINT, HOVERED_TAB_TINT, Measure, Metrics,
    PRESSED_BUTTON_TINT, fit_title,
    layout::{Hit, Layout, tab_content},
};
use crate::{
    paint::{Area, Corner, Icon, Paint, collect_colors, override_colors},
    tab_icon::TabIcon,
};

/// タブ一つに描くもの。
#[derive(Clone, Copy, Debug)]
pub struct TabView<'a> {
    pub title: &'a str,
    /// 表示していない間にベルが鳴った。タイトルの前に印を付ける。
    pub bell: bool,
    /// タイトルの左に出すアイコン。
    pub icon: &'a TabIcon,
}

/// Windows のシステムの色（`GetSysColor`）。タブバーの既定の色（[`Palette::from_system`]）を決める。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SystemColors {
    /// コントロールの背景（`COLOR_BTNFACE`）。
    pub face: Rgb,
    /// コントロールの文字（`COLOR_BTNTEXT`）。
    pub text: Rgb,
    /// 薄い文字（`COLOR_GRAYTEXT`）。
    pub gray_text: Rgb,
    /// コントロールの暗い縁（`COLOR_BTNSHADOW`）。
    pub shadow: Rgb,
    /// ウィンドウの背景（`COLOR_WINDOW`）。
    pub window: Rgb,
    /// ウィンドウの文字（`COLOR_WINDOWTEXT`）。
    pub window_text: Rgb,
}

/// タブバーの色。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Palette {
    /// タブバーの背景。表示していないタブも、マウスを載せていなければこの色のまま。
    pub bar: Rgb,
    /// 表示しているタブ。
    pub active_tab: Rgb,
    /// 表示しているタブの左・右・上の縁の線。
    pub active_border: Rgb,
    /// 表示しているタブのタイトルと閉じるボタンの「×」。
    pub active_title: Rgb,
    /// 表示していないタブのタイトルと閉じるボタンの「×」、マウスを載せている「+」「▾」「≡」。
    pub inactive_title: Rgb,
    /// 表示していないタブの間の区切りの線。
    pub separator: Rgb,
    /// マウスを載せている表示していないタブ。
    pub hovered_tab: Rgb,
    /// マウスを載せていない、新しいタブのボタンの「+」、プロファイルの一覧のボタンの「▾」、メニューの
    /// ボタンの「≡」。
    pub idle_mark: Rgb,
    /// 表示していない間にベルが鳴ったタブの印。
    pub bell: Rgb,
}

impl Palette {
    /// Windows のシステムの色から決める既定の色。背景と表示していないタブはコントロールの背景の色を文字の
    /// 色へ少し寄せた色、表示しているタブはウィンドウの背景と文字の色で、縁の線はコントロールの暗い縁の色。
    /// マウスを載せているタブは、背景とウィンドウの背景の間の色。
    pub fn from_system(system: SystemColors) -> Self {
        let bar = system.face.mix(system.text, BAR_TINT);
        Self {
            bar,
            active_tab: system.window,
            active_border: system.shadow,
            active_title: system.window_text,
            inactive_title: system.text,
            separator: system.shadow,
            hovered_tab: bar.mix(system.window, HOVERED_TAB_TINT),
            idle_mark: system.gray_text,
            bell: BELL_COLOR,
        }
    }

    /// 設定の色 `colors`（[`TAB_BAR_COLOR_KEYS`] の順）のうち、書いた色だけで上書きした色。
    pub fn with_colors(mut self, colors: &[Option<Rgb>]) -> Self {
        override_colors(self.fields_mut(), colors);
        self
    }

    /// 色の並び。[`TAB_BAR_COLOR_KEYS`] の順。
    pub fn colors(&self) -> [Rgb; TAB_BAR_COLOR_KEYS.len()] {
        let mut copy = *self;
        collect_colors(copy.fields_mut())
    }

    /// 色の欄。[`TAB_BAR_COLOR_KEYS`] の順。
    fn fields_mut(&mut self) -> [&mut Rgb; TAB_BAR_COLOR_KEYS.len()] {
        [
            &mut self.bar,
            &mut self.active_tab,
            &mut self.active_border,
            &mut self.active_title,
            &mut self.inactive_title,
            &mut self.separator,
            &mut self.hovered_tab,
            &mut self.idle_mark,
            &mut self.bell,
        ]
    }

    /// 色 `base` の上に、色 `ink` の印で描くボタンの丸い背景の色。`pressed` なら押しているとき、
    /// そうでなければマウスを載せているとき。
    fn button(&self, base: Rgb, ink: Rgb, pressed: bool) -> Rgb {
        let tint = if pressed {
            PRESSED_BUTTON_TINT
        } else {
            HOVERED_BUTTON_TINT
        };
        base.mix(ink, tint)
    }
}

/// マウスを載せているものと、押しているもの。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Highlight {
    pub hovered: Option<Hit>,
    /// 左ボタンを押したときにマウスを載せていたもの。マウスを載せているときだけ、押している色で描く。
    pub pressed: Option<Hit>,
}

impl Highlight {
    /// `hit` の丸い背景の色を決めるとき、押しているか。マウスを載せていなければ `None`。
    fn state(&self, hit: Hit) -> Option<bool> {
        (self.hovered == Some(hit)).then_some(self.pressed == Some(hit))
    }

    /// マウスを載せているタブの位置。
    fn hovered_tab(&self) -> Option<usize> {
        self.hovered.and_then(Hit::tab)
    }
}

/// `layout` のとおりに並べたタブバーに描くもの。`tabs` は端末の並びの順のタブ、`active` は表示して
/// いるタブの位置、`highlight` はマウスを載せているものと押しているもの。色は `colors`、タイトルの幅は
/// `measure` で測る。閉じるボタンは、表示しているタブと
/// マウスを載せているタブにだけ出す。描くのはタブバーの高さまでで、その下の端末の画面の余白には描かない。
pub fn paints(
    layout: &Layout,
    tabs: &[TabView],
    active: usize,
    highlight: Highlight,
    colors: &Palette,
    measure: &impl Measure,
) -> Vec<Paint> {
    let metrics = layout.metrics;
    let hovered = highlight.hovered_tab();
    let raised = |index: usize| index == active || Some(index) == hovered;
    let mut paints = vec![Paint::Fill {
        area: Area {
            x: 0,
            y: 0,
            width: layout.width,
            height: metrics.bar_height(),
        },
        color: colors.bar,
    }];
    for pair in layout.tabs.windows(2) {
        let [left, right] = pair else { continue };
        if raised(left.index) || raised(right.index) {
            continue;
        }
        let (width, height) = metrics.separator();
        let area = right.area;
        paints.push(Paint::Fill {
            area: Area {
                x: area.x.saturating_sub(width / 2),
                y: area.y + (area.height - height) / 2,
                width,
                height,
            },
            color: colors.separator,
        });
    }
    if let Some(tab) = layout
        .tabs
        .iter()
        .find(|tab| Some(tab.index) == hovered && tab.index != active)
    {
        push_raised_tab(
            &mut paints,
            tab.area,
            metrics.corner_radius(),
            colors.hovered_tab,
        );
    }
    if let Some(tab) = layout.tabs.iter().find(|tab| tab.index == active) {
        push_active_tab(&mut paints, metrics, tab.area, colors);
    }
    for slot in &layout.tabs {
        let Some(tab) = tabs.get(slot.index) else {
            continue;
        };
        let is_active = slot.index == active;
        let (tab_color, title_color) = if is_active {
            (colors.active_tab, colors.active_title)
        } else if Some(slot.index) == hovered {
            (colors.hovered_tab, colors.inactive_title)
        } else {
            (colors.bar, colors.inactive_title)
        };
        let content = tab_content(metrics, slot, tab.bell, measure.height());
        if let Some(area) = content.icon {
            paints.push(Paint::TabIcon {
                icon: tab.icon.clone(),
                area,
            });
        }
        if let Some(mark) = content.mark {
            paints.push(Paint::Icon {
                icon: Icon::Dot,
                area: mark,
                color: colors.bell,
            });
        }
        let title = fit_title(tab.title, content.title_width, measure);
        if !title.is_empty() {
            paints.push(Paint::Label {
                x: content.title_x,
                y: content.text_y,
                text: title,
                color: title_color,
            });
        }
        if raised(slot.index) {
            push_button(
                &mut paints,
                colors,
                Button {
                    area: slot.close,
                    mark: (Icon::Close, metrics.close_mark(), title_color),
                    state: highlight.state(Hit::Close(slot.index)),
                    base: tab_color,
                    ink: title_color,
                },
            );
        }
    }
    for (hit, area, icon) in [
        (Hit::NewTab, layout.new_tab, Icon::Plus),
        (Hit::Profiles, layout.profiles, Icon::Chevron),
        (Hit::Menu, layout.menu, Icon::Menu),
    ] {
        let mark_color = if highlight.hovered == Some(hit) {
            colors.inactive_title
        } else {
            colors.idle_mark
        };
        push_button(
            &mut paints,
            colors,
            Button {
                area,
                mark: (icon, metrics.button_mark(), mark_color),
                state: highlight.state(hit),
                base: colors.bar,
                ink: colors.inactive_title,
            },
        );
    }
    paints
}

/// タブバーのボタン（×、+、▾、≡）一つ。
struct Button {
    /// ボタンの矩形。マウスを載せていれば、ここに丸い背景を描く。
    area: Area,
    /// 真ん中に描く印と、その一辺と色。
    mark: (Icon, u32, Rgb),
    /// マウスを載せていれば、押しているか。
    state: Option<bool>,
    /// ボタンの下の色。丸い背景の色は、ここから `ink` へ寄せて決める。
    base: Rgb,
    /// ボタンの下の背景に置く文字の色。
    ink: Rgb,
}

fn push_button(paints: &mut Vec<Paint>, colors: &Palette, button: Button) {
    let Button {
        area,
        mark: (icon, mark_size, mark_color),
        state,
        base,
        ink,
    } = button;
    if let Some(pressed) = state {
        paints.push(Paint::Icon {
            icon: Icon::Dot,
            area,
            color: colors.button(base, ink, pressed),
        });
    }
    let size = mark_size.min(area.width).min(area.height);
    paints.push(Paint::Icon {
        icon,
        area: Area {
            x: area.x + (area.width - size) / 2,
            y: area.y + (area.height - size) / 2,
            width: size,
            height: size,
        },
        color: mark_color,
    });
}

/// 矩形 `area` に、表示しているタブを描く。左・右・上の縁の線を矩形の内側に引き、その内側を
/// 表示しているタブの色で塗る。縁の線の角は、タブの丸めた角に沿わせる（内側の角の半径を線の太さの
/// 分だけ小さくし、同じ中心の円にする）。下の辺には線を引かない。
fn push_active_tab(paints: &mut Vec<Paint>, metrics: Metrics, area: Area, colors: &Palette) {
    let radius = metrics.corner_radius();
    let border = metrics.active_border();
    push_raised_tab(paints, area, radius, colors.active_border);
    let inner = Area {
        x: area.x + border,
        y: area.y + border,
        width: area.width.saturating_sub(border * 2),
        height: area.height.saturating_sub(border),
    };
    push_raised_tab(
        paints,
        inner,
        radius.saturating_sub(border),
        colors.active_tab,
    );
}

/// 矩形 `area` に、上の角を半径 `radius` で丸めたタブを `color` で描く。下の辺はまっすぐ。
fn push_raised_tab(paints: &mut Vec<Paint>, area: Area, radius: u32, color: Rgb) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    if radius == 0 || area.width < radius * 2 || area.height < radius {
        paints.push(Paint::Fill { area, color });
        return;
    }
    let square = |x: u32, y: u32| Area {
        x,
        y,
        width: radius,
        height: radius,
    };
    paints.push(Paint::Fill {
        area: Area {
            x: area.x + radius,
            y: area.y,
            width: area.width - radius * 2,
            height: radius,
        },
        color,
    });
    paints.push(Paint::Fill {
        area: Area {
            y: area.y + radius,
            height: area.height - radius,
            ..area
        },
        color,
    });
    paints.push(Paint::Icon {
        icon: Icon::Corner(Corner::TopLeft),
        area: square(area.x, area.y),
        color,
    });
    paints.push(Paint::Icon {
        icon: Icon::Corner(Corner::TopRight),
        area: square(area.right() - radius, area.y),
        color,
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tab_bar::{
        fixtures::{FixedWidth, METRICS, WIDTH, square},
        layout::layout,
    };

    const fn gray(level: u8) -> Rgb {
        Rgb {
            r: level,
            g: level,
            b: level,
        }
    }

    /// Windows 11 の既定に近いシステムの色。色で描くものを見分けられるよう、どれも違う色にする。
    const SYSTEM: SystemColors = SystemColors {
        face: gray(0xf0),
        text: gray(0x20),
        gray_text: gray(0x6d),
        shadow: gray(0xa0),
        window: gray(0xfe),
        window_text: gray(0x10),
    };

    /// タブのアイコン。
    static ICON: TabIcon = TabIcon::without_profile();

    /// 表示しているタブの色。
    const WINDOW: Rgb = SYSTEM.window;

    /// 表示しているタブのタイトルの色。
    const WINDOW_TEXT: Rgb = SYSTEM.window_text;

    fn palette() -> Palette {
        Palette::from_system(SYSTEM)
    }

    fn titles(paints: &[Paint]) -> Vec<(u32, u32, String, Rgb)> {
        paints
            .iter()
            .filter_map(|paint| match paint {
                Paint::Label { x, y, text, color } => Some((*x, *y, text.clone(), *color)),
                _ => None,
            })
            .collect()
    }

    fn dots(paints: &[Paint]) -> Vec<Area> {
        paints
            .iter()
            .filter_map(|paint| match paint {
                Paint::Icon {
                    icon: Icon::Dot,
                    area,
                    color,
                } if *color == BELL_COLOR => Some(*area),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn a_bell_mark_survives_a_long_title() {
        let powershell = r"C:\WINDOWS\System32\WindowsPowerShell\v1.0\powershell.exe";
        let tabs = [
            TabView {
                title: powershell,
                bell: true,
                icon: &ICON,
            },
            TabView {
                title: powershell,
                bell: false,
                icon: &ICON,
            },
            TabView {
                title: "zsh",
                bell: false,
                icon: &ICON,
            },
        ];
        let layout = layout(METRICS, WIDTH, 3, 2);
        let paints = paints(
            &layout,
            &tabs,
            2,
            Highlight::default(),
            &palette(),
            &FixedWidth,
        );
        assert_eq!(dots(&paints).len(), 1);
        assert!(dots(&paints)[0].right() <= 540);
        let titles: Vec<String> = titles(&paints)
            .into_iter()
            .map(|(_, _, text, ..)| text)
            .collect();
        // タブの幅は 478。印のあるタブは 354 ピクセル、ないタブは 378 ピクセルに収める。
        assert_eq!(
            titles,
            vec![
                r"C:\WINDOWS\System32\WindowsPowerSh…",
                r"C:\WINDOWS\System32\WindowsPowerShel…",
                "zsh",
            ]
        );
    }

    #[test]
    fn many_tabs_keep_their_bell_marks() {
        let tabs: Vec<TabView> = (0..40)
            .map(|index| TabView {
                title: "abcdefghijklmnopqrstuvwxyz",
                bell: index % 2 == 0,
                icon: &ICON,
            })
            .collect();
        let layout = layout(METRICS, WIDTH, tabs.len(), 3);
        let paints = paints(
            &layout,
            &tabs,
            3,
            Highlight::default(),
            &palette(),
            &FixedWidth,
        );
        // 描く 10 個のうち、偶数番の 5 個に印。
        assert_eq!(dots(&paints).len(), 5);
        for (slot, dot) in layout.tabs().iter().step_by(2).zip(dots(&paints)) {
            assert!(dot.x >= slot.area.x && dot.right() <= slot.area.right());
        }
    }

    /// 設定のキーと、その色の欄。
    type ColorField = (&'static str, fn(&Palette) -> Rgb);

    const COLOR_FIELDS: [ColorField; 9] = [
        ("bar", |p| p.bar),
        ("active_tab", |p| p.active_tab),
        ("active_border", |p| p.active_border),
        ("active_title", |p| p.active_title),
        ("inactive_title", |p| p.inactive_title),
        ("separator", |p| p.separator),
        ("hovered_tab", |p| p.hovered_tab),
        ("idle_mark", |p| p.idle_mark),
        ("bell", |p| p.bell),
    ];

    #[test]
    fn the_colour_fields_follow_the_order_of_the_keys() {
        let palette = palette();
        let colors = palette.colors();
        for (index, (name, field)) in COLOR_FIELDS.iter().enumerate() {
            assert_eq!(TAB_BAR_COLOR_KEYS[index], *name);
            assert_eq!(colors[index], field(&palette), "{name}");
        }
        let written = Rgb { r: 1, g: 2, b: 3 };
        let mut overrides = [None; TAB_BAR_COLOR_KEYS.len()];
        overrides[TAB_BAR_COLOR_KEYS.len() - 1] = Some(written);
        assert_eq!(palette.with_colors(&overrides).bell, written);
    }

    #[test]
    fn template_examples_are_the_windows_11_defaults() {
        let windows_11 = Palette::from_system(SystemColors {
            face: gray(0xf0),
            text: gray(0),
            gray_text: gray(0x6d),
            shadow: gray(0xa0),
            window: gray(0xff),
            window_text: gray(0),
        });
        // バーの背景（#cccccc）はコントロールの背景より少し濃く、ホバー中のタブ（#e6e6e6）はバーより明るい。
        let examples: Vec<String> = COLOR_FIELDS
            .iter()
            .map(|(_, field)| fantermo_core::config::format_color(field(&windows_11)))
            .collect();
        assert_eq!(
            examples,
            [
                "#cccccc", "#ffffff", "#a0a0a0", "#000000", "#000000", "#a0a0a0", "#e6e6e6",
                "#6d6d6d", "#f0b030"
            ]
        );
    }

    #[test]
    fn default_colours_come_from_the_system() {
        let colors = palette();
        // 背景はコントロールの背景の色を、文字の色へ 15% 寄せる。
        assert_eq!(colors.bar, SYSTEM.face.mix(SYSTEM.text, BAR_TINT));
        assert!(colors.bar.r < SYSTEM.face.r && colors.bar.r > SYSTEM.text.r);
        assert_eq!(
            (colors.active_tab, colors.active_title),
            (WINDOW, WINDOW_TEXT)
        );
        assert_eq!(colors.active_border, SYSTEM.shadow);
        assert_eq!(colors.inactive_title, SYSTEM.text);
        assert_eq!(colors.separator, SYSTEM.shadow);
        assert_eq!(colors.idle_mark, SYSTEM.gray_text);
        assert_eq!(colors.bell, BELL_COLOR);
        // マウスを載せているタブは、背景とウィンドウの背景の間。
        assert!(colors.hovered_tab.r > colors.bar.r && colors.hovered_tab.r < WINDOW.r);
    }

    #[test]
    fn the_active_tab_rises_in_the_window_colour() {
        let tabs = [
            TabView {
                title: "one",
                bell: false,
                icon: &ICON,
            },
            TabView {
                title: "端末",
                bell: false,
                icon: &ICON,
            },
        ];
        let layout = layout(METRICS, WIDTH, 2, 1);
        let colors = palette();
        let paints = paints(
            &layout,
            &tabs,
            1,
            Highlight::default(),
            &colors,
            &FixedWidth,
        );
        // 背景はタブバーの全体で、その下の端末の画面の余白は含まない。
        assert_eq!(
            paints[0],
            Paint::Fill {
                area: Area {
                    x: 0,
                    y: 0,
                    width: WIDTH,
                    height: 52
                },
                color: colors.bar
            }
        );
        let icons = |wanted: Rgb| -> Vec<(Icon, Area)> {
            paints
                .iter()
                .filter_map(|paint| match paint {
                    Paint::Icon { icon, area, color } if *color == wanted => Some((*icon, *area)),
                    _ => None,
                })
                .collect()
        };
        let fills = |wanted: Rgb| -> Vec<Area> {
            paints
                .iter()
                .filter_map(|paint| match paint {
                    Paint::Fill { area, color } if *color == wanted => Some(*area),
                    _ => None,
                })
                .collect()
        };
        // 表示しているタブは、まず縁の線の色でタブの矩形の全体を、上の角を半径 10 で丸めて塗る。
        assert_eq!(
            icons(colors.active_border),
            vec![
                (Icon::Corner(Corner::TopLeft), square(540, 4, 10)),
                (Icon::Corner(Corner::TopRight), square(1010, 4, 10)),
            ]
        );
        assert_eq!(
            fills(colors.active_border),
            vec![
                Area {
                    x: 550,
                    y: 4,
                    width: 460,
                    height: 10
                },
                Area {
                    x: 540,
                    y: 14,
                    width: 480,
                    height: 38
                },
            ]
        );
        // その内側を、左・右・上を線の太さ 2 だけ空けてウィンドウの背景の色で塗る。角は同じ中心
        // (550, 14) と (1010, 14) の半径 8 の円。下の辺はまっすぐで、タブバーの下の辺まで（下には線が
        // ない）。
        assert_eq!(
            icons(WINDOW),
            vec![
                (Icon::Corner(Corner::TopLeft), square(542, 6, 8)),
                (Icon::Corner(Corner::TopRight), square(1010, 6, 8)),
            ]
        );
        assert_eq!(
            fills(WINDOW),
            vec![
                Area {
                    x: 550,
                    y: 6,
                    width: 460,
                    height: 8
                },
                Area {
                    x: 542,
                    y: 14,
                    width: 476,
                    height: 38
                },
            ]
        );
        // 縁の線を先に、ウィンドウの背景の色を後に描く。
        let first_window = paints
            .iter()
            .position(|paint| matches!(paint, Paint::Fill { color, .. } if *color == WINDOW));
        let last_border = paints.iter().rposition(
            |paint| matches!(paint, Paint::Icon { color, .. } if *color == colors.active_border),
        );
        assert!(last_border < first_window);
        // どれもタブバーの中に描く。
        assert!(paints.iter().all(|paint| match paint {
            Paint::Fill { area, .. }
            | Paint::Translucent { area, .. }
            | Paint::Icon { area, .. }
            | Paint::TabIcon { area, .. } => {
                area.bottom() <= 52
            }
            Paint::Label { y, .. } => y + FixedWidth.height() <= 52,
        }));
        // アイコンは、タブごとにタイトルの左に。
        let tab_icons: Vec<Area> = paints
            .iter()
            .filter_map(|paint| match paint {
                Paint::TabIcon { icon, area } if *icon == ICON => Some(*area),
                _ => None,
            })
            .collect();
        assert_eq!(tab_icons, vec![square(72, 10, 36), square(552, 10, 36)]);
        // タイトルは一つの文字列として、アイコンの右、タブの高さの真ん中に置く。表示しているタブの
        // タイトルはウィンドウの文字の色、ほかはコントロールの文字の色。
        assert_eq!(
            titles(&paints),
            vec![
                (118, 12, "one".to_owned(), SYSTEM.text),
                (598, 12, "端末".to_owned(), WINDOW_TEXT),
            ]
        );
        // メニューのボタンの「≡」は、一辺 16 でボタンの真ん中に、薄い文字の色で。
        assert_eq!(
            icons_of(&paints, Icon::Menu),
            vec![(square(22, 20, 16), SYSTEM.gray_text)]
        );
        // プロファイルの一覧のボタンの「▾」も、同じ大きさと色で新しいタブのボタンのすぐ右に。
        let profiles = layout.profiles;
        assert_eq!(profiles.x, layout.new_tab.right());
        assert_eq!(
            icons_of(&paints, Icon::Chevron),
            vec![(
                square(profiles.x + 10, profiles.y + 10, 16),
                SYSTEM.gray_text
            )]
        );
    }

    #[test]
    fn separators_stand_between_tabs_that_are_not_shown() {
        let tabs = [TabView {
            title: "a",
            bell: false,
            icon: &ICON,
        }; 4];
        let layout = layout(METRICS, WIDTH, 4, 1);
        let colors = palette();
        let separators = |highlight: Highlight| -> Vec<u32> {
            paints(&layout, &tabs, 1, highlight, &colors, &FixedWidth)
                .iter()
                .filter_map(|paint| match paint {
                    // 表示しているタブの縁の線も同じ色なので、太さ 2 の線だけを数える。
                    Paint::Fill { area, color }
                        if *color == colors.separator && area.width == 2 =>
                    {
                        Some(area.x)
                    }
                    _ => None,
                })
                .collect()
        };
        // 表示している 1 番の左右には引かない。2 番と 3 番の間だけ。太さ 2 の線を境目の真ん中に。
        assert_eq!(
            separators(Highlight::default()),
            vec![layout.tabs()[3].area.x - 1]
        );
        // マウスを載せているタブの左右にも引かない。
        let hovered = Highlight {
            hovered: Some(Hit::Close(3)),
            pressed: None,
        };
        assert!(separators(hovered).is_empty());
    }

    /// `paints` のうち、`icon` を描くものの矩形と色。
    fn icons_of(paints: &[Paint], wanted: Icon) -> Vec<(Area, Rgb)> {
        paints
            .iter()
            .filter_map(|paint| match paint {
                Paint::Icon { icon, area, color } if *icon == wanted => Some((*area, *color)),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn close_buttons_show_on_the_active_and_the_hovered_tab() {
        let tabs = [TabView {
            title: "a",
            bell: false,
            icon: &ICON,
        }; 3];
        let layout = layout(METRICS, WIDTH, 3, 0);
        let colors = palette();
        let close_marks = |highlight: Highlight| -> Vec<u32> {
            icons_of(
                &paints(&layout, &tabs, 0, highlight, &colors, &FixedWidth),
                Icon::Close,
            )
            .iter()
            .map(|(area, _)| area.x)
            .collect()
        };
        // 「×」は一辺 16 で、ボタンの真ん中に。
        let close = layout.tabs()[0].close;
        assert_eq!(close_marks(Highlight::default()), vec![close.x + 8]);
        let hovered = Highlight {
            hovered: Some(Hit::Tab(2)),
            pressed: None,
        };
        assert_eq!(
            close_marks(hovered),
            vec![close.x + 8, layout.tabs()[2].close.x + 8]
        );
        // マウスを載せているタブも、表示しているタブと同じく上の角を丸めて描く。
        let paints = paints(&layout, &tabs, 0, hovered, &colors, &FixedWidth);
        let corners = icons_of(&paints, Icon::Corner(Corner::TopLeft));
        assert_eq!(
            corners,
            vec![
                (square(layout.tabs()[2].area.x, 4, 10), colors.hovered_tab),
                (square(60, 4, 10), colors.active_border),
                (square(62, 6, 8), WINDOW),
            ]
        );
    }

    #[test]
    fn buttons_under_the_pointer_get_a_round_background() {
        let tabs = [TabView {
            title: "a",
            bell: false,
            icon: &ICON,
        }; 2];
        let layout = layout(METRICS, WIDTH, 2, 0);
        let colors = palette();
        let backgrounds = |highlight: Highlight| -> Vec<(Area, Rgb)> {
            icons_of(
                &paints(&layout, &tabs, 0, highlight, &colors, &FixedWidth),
                Icon::Dot,
            )
        };
        assert_eq!(backgrounds(Highlight::default()), vec![]);
        let close = layout.tabs()[0].close;
        let over_close = Highlight {
            hovered: Some(Hit::Close(0)),
            pressed: None,
        };
        // 表示しているタブの上では、ウィンドウの背景の色をウィンドウの文字の色へ寄せる。
        assert_eq!(
            backgrounds(over_close),
            vec![(close, colors.button(WINDOW, WINDOW_TEXT, false))]
        );
        // 押したままボタンの上にあれば押している色、外へ出れば背景を描かない。
        let pressed = Highlight {
            pressed: Some(Hit::Close(0)),
            ..over_close
        };
        assert_eq!(
            backgrounds(pressed),
            vec![(close, colors.button(WINDOW, WINDOW_TEXT, true))]
        );
        let pressed_elsewhere = Highlight {
            hovered: Some(Hit::Tab(0)),
            pressed: Some(Hit::Close(0)),
        };
        assert_eq!(backgrounds(pressed_elsewhere), vec![]);
        // 新しいタブのボタン。背景の上では、背景の色をコントロールの文字の色へ寄せる。「+」は一辺 16 で
        // 真ん中に、マウスを載せていればコントロールの文字の色。
        let over_new_tab = Highlight {
            hovered: Some(Hit::NewTab),
            pressed: None,
        };
        let text = SYSTEM.text;
        assert_eq!(
            backgrounds(over_new_tab),
            vec![(layout.new_tab, colors.button(colors.bar, text, false))]
        );
        let plus = icons_of(
            &paints(&layout, &tabs, 0, over_new_tab, &colors, &FixedWidth),
            Icon::Plus,
        );
        assert_eq!(
            plus,
            vec![(
                square(layout.new_tab.x + 10, layout.new_tab.y + 10, 16),
                text
            )]
        );
        assert_ne!(
            colors.button(WINDOW, WINDOW_TEXT, false),
            colors.button(WINDOW, WINDOW_TEXT, true)
        );
    }
}
