//! 画像（kitty graphics protocol と sixel）の置き方と、描画のための取り出し方。

use std::{borrow::Cow, time::Duration};

use libghostty_vt::{
    Terminal,
    kitty::graphics::ImageFormat,
    render::{ImageIterator, ImageLayer as VtImageLayer, ImageProtocol, Snapshot},
};

use super::{Vt, VtError};
use crate::{
    backend::GridSize,
    frame::{ImageKey, ImageLayer, ImagePixels, ImageView},
    sixel::SixelImage,
};

/// カーソルを同じ桁のまま一行下へ動かす（IND）。下端ではスクロールする。
const INDEX: &[u8] = b"\x1bD";

impl Vt {
    /// `key` の画像のピクセル。保管にその画像がないか、`key` のあとでピクセルが変わった、または
    /// まだ届いていなければ `None`。
    pub fn image_pixels(&self, key: &ImageKey) -> Result<Option<ImagePixels<'_>>, VtError> {
        let graphics = self.terminal.kitty_graphics()?;
        let Some(image) = graphics.image(key.id) else {
            return Ok(None);
        };
        if image.generation()? != key.generation {
            return Ok(None);
        }
        let Some(data) = image.data()? else {
            return Ok(None);
        };
        Ok(rgba_pixels(
            image.format()?,
            data,
            image.width()?,
            image.height()?,
        ))
    }

    /// `key` の画像が、同じピクセルのまま表示している画面の保管にある。
    pub fn image_is_stored(&self, key: &ImageKey) -> Result<bool, VtError> {
        let graphics = self.terminal.kitty_graphics()?;
        let Some(image) = graphics.image(key.id) else {
            return Ok(false);
        };
        Ok(image.generation()? == key.generation)
    }

    /// 再生中の画像のアニメーションを進める。描き直す前に呼ぶ。`now` は同じ時計で測った時刻。次のコマ
    /// までの時間を返す。再生中のものがなければ `None`。
    pub fn tick_image_animations(&mut self, now: Duration) -> Result<Option<Duration>, VtError> {
        let now_ms = u64::try_from(now.as_millis()).unwrap_or(u64::MAX);
        let next_ms = self.terminal.kitty_graphics_animation_tick(now_ms)?;
        Ok(next_ms.map(|next_ms| Duration::from_millis(next_ms.saturating_sub(now_ms))))
    }
}

/// sixel の画像を kitty graphics protocol の転送にして端末に置く。位置の追跡、スクロール、消去、代替画面は
/// kitty graphics protocol の画像と同じになる。
///
/// 画像は文字より下に置き、あとから書いた文字が画像の上に見えるようにする。不透明な画像なら、覆うセルの
/// 文字を消して画像を見せる。
///
/// xterm と違い、文字を書いてもそのセルの画像は消えない（kitty graphics protocol の画像の配置は文字に
/// 反応しない）。空白で画像を消すアプリでは古い画像が残る。`CSI 2 J` では消える。
///
/// `scrolling` なら、カーソルの位置に置き、カーソルを画像の最後の行の同じ桁へ進める（xterm と同じ）。下端で
/// は画面がスクロールする。そうでなければ（DECSDM）、画面の左上に置いてカーソルを動かさない。
pub(super) fn place_sixel(
    terminal: &mut Terminal<'static, 'static>,
    size: GridSize,
    image: &SixelImage,
    scrolling: bool,
) {
    let rows = image.height.div_ceil(size.cell_height_px.max(1));
    let cols = image.width.div_ceil(size.cell_width_px.max(1));
    let erase = image
        .is_opaque()
        .then(|| format!("\x1b[{cols}X").into_bytes());
    let erase = erase.as_deref().unwrap_or_default();

    if scrolling {
        image.write_kitty_transmission(&mut |apc| terminal.vt_write(apc));
        terminal.vt_write(erase);
        for _ in 1..rows {
            terminal.vt_write(INDEX);
            terminal.vt_write(erase);
        }
        return;
    }

    let cursor = terminal
        .cursor_x()
        .and_then(|col| Ok((col, terminal.cursor_y()?)));
    terminal.vt_write(b"\x1b[H");
    image.write_kitty_transmission(&mut |apc| terminal.vt_write(apc));
    for row in 0..rows.min(u32::from(size.rows)) {
        terminal.vt_write(format!("\x1b[{};1H", row + 1).as_bytes());
        terminal.vt_write(erase);
    }
    match cursor {
        Ok((col, row)) => terminal.vt_write(format!("\x1b[{};{}H", row + 1, col + 1).as_bytes()),
        Err(e) => tracing::warn!("failed to read the cursor before placing a sixel: {e}"),
    }
}

/// `snapshot` の表示している範囲の画像を、層ごとに下から描く順に並べる。同じ高さなら画像の番号の順
/// （Ghostty と同じ）。
pub(super) fn image_views(
    images: &mut ImageIterator<'static>,
    snapshot: &Snapshot<'static, '_>,
) -> Result<Vec<ImageView>, VtError> {
    let mut views = Vec::new();
    let mut placements = images.update(snapshot)?;
    while let Some(placement) = placements.next() {
        // 新しいバージョンで増えた種類は、エラーとして返ることもある。読めるものだけを描く。
        let Ok(ImageProtocol::Kitty) = placement.protocol() else {
            continue;
        };
        views.push(ImageView {
            key: ImageKey {
                id: placement.image_id()?,
                generation: placement.image_generation()?,
            },
            layer: match placement.layer()? {
                VtImageLayer::BelowBackground => ImageLayer::BelowBackground,
                VtImageLayer::BelowText => ImageLayer::BelowText,
                VtImageLayer::AboveText => ImageLayer::AboveText,
            },
            z: placement.z()?,
            col: placement.viewport_col()?,
            row: placement.viewport_row()?,
            offset_x: placement.cell_offset_x()?,
            offset_y: placement.cell_offset_y()?,
            width: placement.pixel_width()?,
            height: placement.pixel_height()?,
            source_x: placement.source_x()?,
            source_y: placement.source_y()?,
            source_width: placement.source_width()?,
            source_height: placement.source_height()?,
        });
    }
    views.sort_by_key(|view| (view.layer, view.z, view.key.id));
    Ok(views)
}

/// 大きさ `width` × `height` の `format` のピクセルを RGBA にする。バイト数が足りない、または
/// RGBA にできない形式なら `None`。
pub(super) fn rgba_pixels(
    format: ImageFormat,
    data: &[u8],
    width: u32,
    height: u32,
) -> Option<ImagePixels<'_>> {
    let pixels = (width as usize).checked_mul(height as usize)?;
    let channels = match format {
        ImageFormat::Gray => 1,
        ImageFormat::GrayAlpha => 2,
        ImageFormat::Rgb => 3,
        ImageFormat::Rgba => 4,
        _ => return None,
    };
    let data = data.get(..pixels.checked_mul(channels)?)?;
    let rgba = match format {
        ImageFormat::Rgba => Cow::Borrowed(data),
        _ => Cow::Owned(
            data.chunks_exact(channels)
                .flat_map(|pixel| match *pixel {
                    [gray] => [gray, gray, gray, u8::MAX],
                    [gray, alpha] => [gray, gray, gray, alpha],
                    [r, g, b] => [r, g, b, u8::MAX],
                    _ => unreachable!("channels is 1, 2 or 3 here"),
                })
                .collect(),
        ),
    };
    Some(ImagePixels {
        width,
        height,
        rgba,
    })
}
