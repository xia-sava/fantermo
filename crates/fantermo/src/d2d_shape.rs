//! [`Shape`] を Direct2D の描画先に描く。グリフアトラスとタブのアイコンが使う。

use windows::Win32::Graphics::Direct2D::{
    Common::{
        D2D_RECT_F, D2D_SIZE_F, D2D1_FIGURE_BEGIN_FILLED, D2D1_FIGURE_BEGIN_HOLLOW,
        D2D1_FIGURE_END_CLOSED, D2D1_FIGURE_END_OPEN,
    },
    D2D1_ARC_SEGMENT, D2D1_ARC_SIZE_SMALL, D2D1_QUADRATIC_BEZIER_SEGMENT,
    D2D1_SWEEP_DIRECTION_CLOCKWISE, D2D1_SWEEP_DIRECTION_COUNTER_CLOCKWISE, ID2D1Factory1,
    ID2D1PathGeometry1, ID2D1RenderTarget, ID2D1SolidColorBrush,
};
use windows_numerics::Vector2;

use crate::boxdraw::Shape;

/// Direct2D の座標をピクセルと同じにする DPI。
pub const PIXEL_DPI: f32 = 96.0;

/// `shape` を `brush` で `target` に描く。`factory` は `target` を作ったファクトリ、`brush` は `target` が
/// 作ったブラシ。`BeginDraw` と `EndDraw` の間で呼ぶ。
pub fn draw_shape(
    target: &ID2D1RenderTarget,
    factory: &ID2D1Factory1,
    brush: &ID2D1SolidColorBrush,
    shape: &Shape,
) -> windows::core::Result<()> {
    match *shape {
        Shape::Rect {
            left,
            top,
            right,
            bottom,
            alpha,
        } => {
            let rect = D2D_RECT_F {
                left,
                top,
                right,
                bottom,
            };
            // SAFETY: 描画中の target と、同じ target が作ったブラシ。
            unsafe {
                brush.SetOpacity(alpha);
                target.FillRectangle(&rect, brush);
                brush.SetOpacity(1.0);
            }
        }
        Shape::Line { from, to, width } => {
            // SAFETY: 描画中の target と、同じ target が作ったブラシ。
            unsafe {
                target.DrawLine(point(from), point(to), brush, width, None);
            }
        }
        Shape::RoundedCorner {
            from,
            corner,
            to,
            radius,
            width,
        } => {
            let geometry = rounded_corner(factory, from, corner, to, radius)?;
            // SAFETY: 描画中の target と、同じファクトリで作った図形。
            unsafe { target.DrawGeometry(&geometry, brush, width, None) };
        }
        Shape::Wave {
            left,
            right,
            center_y,
            amplitude,
            period,
            width,
        } => {
            let geometry = wave(factory, left, right, center_y, amplitude, period)?;
            // SAFETY: 描画中の target と、同じファクトリで作った図形。
            unsafe { target.DrawGeometry(&geometry, brush, width, None) };
        }
        Shape::Polygon { ref points, alpha } => {
            let geometry = path(factory, points, true)?;
            // SAFETY: 描画中の target と、同じ target が作ったブラシと、同じファクトリで作った図形。
            unsafe {
                brush.SetOpacity(alpha);
                target.FillGeometry(&geometry, brush, None);
                brush.SetOpacity(1.0);
            }
        }
        Shape::Polyline { ref points, width } => {
            let geometry = path(factory, points, false)?;
            // SAFETY: 描画中の target と、同じファクトリで作った図形。
            unsafe { target.DrawGeometry(&geometry, brush, width, None) };
        }
    }
    Ok(())
}

/// `points` を順に結んだ図形。`closed` なら閉じて塗れる形にする。
fn path(
    factory: &ID2D1Factory1,
    points: &[(f32, f32)],
    closed: bool,
) -> windows::core::Result<ID2D1PathGeometry1> {
    let (begin, end) = if closed {
        (D2D1_FIGURE_BEGIN_FILLED, D2D1_FIGURE_END_CLOSED)
    } else {
        (D2D1_FIGURE_BEGIN_HOLLOW, D2D1_FIGURE_END_OPEN)
    };
    let vertices: Vec<Vector2> = points.iter().map(|&p| point(p)).collect();
    // SAFETY: ファクトリは有効で、sink は作った図形にだけ書き、閉じてから図形を使う。
    unsafe {
        let geometry = factory.CreatePathGeometry()?;
        let sink = geometry.Open()?;
        if let [first, rest @ ..] = &vertices[..] {
            sink.BeginFigure(*first, begin);
            sink.AddLines(rest);
            sink.EndFigure(end);
        }
        sink.Close()?;
        Ok(geometry)
    }
}

/// 半周期ごとに二次ベジェ曲線で山と谷をつないだ波線。`left` から上へ向かって始まる。
fn wave(
    factory: &ID2D1Factory1,
    left: f32,
    right: f32,
    center_y: f32,
    amplitude: f32,
    period: f32,
) -> windows::core::Result<ID2D1PathGeometry1> {
    let half = period / 2.0;
    // 二次ベジェ曲線の頂点は、制御点までの高さの半分になる。
    let control = amplitude * 2.0;
    // SAFETY: ファクトリは有効で、sink は作った図形にだけ書き、閉じてから図形を使う。
    unsafe {
        let geometry = factory.CreatePathGeometry()?;
        let sink = geometry.Open()?;
        sink.BeginFigure(point((left, center_y)), D2D1_FIGURE_BEGIN_HOLLOW);
        let mut x = left;
        let mut upward = true;
        while x < right {
            let direction = if upward { -1.0 } else { 1.0 };
            sink.AddQuadraticBezier(&D2D1_QUADRATIC_BEZIER_SEGMENT {
                point1: point((x + half / 2.0, center_y + direction * control)),
                point2: point((x + half, center_y)),
            });
            x += half;
            upward = !upward;
        }
        sink.EndFigure(D2D1_FIGURE_END_OPEN);
        sink.Close()?;
        Ok(geometry)
    }
}

/// `from` から `corner` へ向かい、半径 `radius` の四分円で曲がって `to` へ向かう線。
fn rounded_corner(
    factory: &ID2D1Factory1,
    from: (f32, f32),
    corner: (f32, f32),
    to: (f32, f32),
    radius: f32,
) -> windows::core::Result<ID2D1PathGeometry1> {
    let incoming = unit(from, corner);
    let outgoing = unit(corner, to);
    let arc_start = (
        corner.0 - incoming.0 * radius,
        corner.1 - incoming.1 * radius,
    );
    let arc_end = (
        corner.0 + outgoing.0 * radius,
        corner.1 + outgoing.1 * radius,
    );
    // y が下向きの座標では、向きの外積が正なら右回り。
    let clockwise = incoming.0 * outgoing.1 - incoming.1 * outgoing.0 > 0.0;

    // SAFETY: ファクトリは有効で、sink は作った図形にだけ書き、閉じてから図形を使う。
    unsafe {
        let geometry = factory.CreatePathGeometry()?;
        let sink = geometry.Open()?;
        sink.BeginFigure(point(from), D2D1_FIGURE_BEGIN_HOLLOW);
        sink.AddLine(point(arc_start));
        sink.AddArc(&D2D1_ARC_SEGMENT {
            point: point(arc_end),
            size: D2D_SIZE_F {
                width: radius,
                height: radius,
            },
            rotationAngle: 0.0,
            sweepDirection: if clockwise {
                D2D1_SWEEP_DIRECTION_CLOCKWISE
            } else {
                D2D1_SWEEP_DIRECTION_COUNTER_CLOCKWISE
            },
            arcSize: D2D1_ARC_SIZE_SMALL,
        });
        sink.AddLine(point(to));
        sink.EndFigure(D2D1_FIGURE_END_OPEN);
        sink.Close()?;
        Ok(geometry)
    }
}

fn point((x, y): (f32, f32)) -> Vector2 {
    Vector2 { X: x, Y: y }
}

/// `from` から `to` へ向かう単位ベクトル。
fn unit(from: (f32, f32), to: (f32, f32)) -> (f32, f32) {
    let (dx, dy) = (to.0 - from.0, to.1 - from.1);
    let length = (dx * dx + dy * dy).sqrt().max(f32::EPSILON);
    (dx / length, dy / length)
}
