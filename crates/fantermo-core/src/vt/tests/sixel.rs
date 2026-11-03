use super::*;

/// 大きさ `width` × `height` の、黒で埋めた sixel。
fn black_sixel(width: u32, height: u32) -> Vec<u8> {
    format!("\x1bPq\"1;1;{width};{height}\x1b\\").into_bytes()
}

#[test]
fn a_sixel_becomes_an_image_at_the_cursor_below_the_text() {
    let mut vt = new_vt();
    vt.feed(b"abc");
    vt.feed(b"\x1bP0;1q#1;2;100;0;0#1!2@\x1b\\");
    assert!(vt.take_replies().is_empty());
    let frame = vt.frame().unwrap();
    let [view] = frame.images.as_slice() else {
        panic!("{:?}", frame.images);
    };
    assert_eq!((view.col, view.row), (3, 0));
    assert_eq!((view.width, view.height), (2, 1));
    assert_eq!(view.layer, ImageLayer::BelowText);
    let pixels = vt.image_pixels(&view.key).unwrap().unwrap();
    assert_eq!((pixels.width, pixels.height), (2, 1));
    assert_eq!(pixels.rgba.as_ref(), [255, 0, 0, 255, 255, 0, 0, 255]);
}

#[test]
fn large_sixels_arrive_whole() {
    let mut vt = new_vt();
    vt.feed(b"\x1bPq#1;2;0;100;0#1!300~-!300~\x1b\\");
    let frame = vt.frame().unwrap();
    let pixels = vt.image_pixels(&frame.images[0].key).unwrap().unwrap();
    assert_eq!((pixels.width, pixels.height), (300, 12));
    let (rgba, _) = pixels.rgba.as_chunks::<4>();
    assert!(rgba.iter().all(|pixel| *pixel == [0, 255, 0, 255]));
}

#[test]
fn the_cursor_stops_on_the_last_row_of_the_sixel_in_the_same_column() {
    let mut vt = new_vt();
    vt.feed(b"abc");
    // セルは 8×16。32 ピクセルは 2 行、33 ピクセルは 3 行。
    vt.feed(&black_sixel(16, 32));
    assert_eq!(vt.cursor_cell().unwrap(), (3, 1));
    vt.feed(b"\x1b[H");
    vt.feed(&black_sixel(16, 33));
    assert_eq!(vt.cursor_cell().unwrap(), (0, 2));
    vt.feed(b"\x1b[H");
    vt.feed(&black_sixel(1, 1));
    assert_eq!(vt.cursor_cell().unwrap(), (0, 0));
}

#[test]
fn the_text_after_a_sixel_is_written_at_the_cursor() {
    let mut vt = new_vt();
    let mut data = black_sixel(16, 32);
    data.extend_from_slice(b"\r\nx");
    vt.feed(&data);
    assert_eq!(vt.cursor_cell().unwrap(), (1, 2));
    assert_eq!(vt.plain_text().unwrap().trim_start_matches('\n'), "x");
}

#[test]
fn a_sixel_at_the_bottom_scrolls_the_screen() {
    let mut vt = new_vt();
    vt.feed(b"top\x1b[5;1H");
    vt.feed(&black_sixel(16, 48));
    assert_eq!(vt.cursor_cell().unwrap(), (0, 4));
    let frame = vt.frame().unwrap();
    assert_eq!(frame.images[0].row, 2);
    assert_eq!(vt.scroll_position().unwrap().total, 7);
}

#[test]
fn a_sixel_taller_than_the_screen_keeps_its_top_in_the_scrollback() {
    let mut vt = new_vt();
    vt.feed(&black_sixel(16, 16 * 8));
    assert_eq!(vt.cursor_cell().unwrap(), (0, 4));
    assert_eq!(vt.frame().unwrap().images[0].row, -3);
}

#[test]
fn an_opaque_sixel_erases_the_text_it_covers_and_later_text_shows() {
    let mut vt = new_vt();
    vt.feed(b"abcdef\r\nghijkl\r\nmnopqr\x1b[H");
    vt.feed(&black_sixel(16, 32));
    assert_eq!(vt.plain_text().unwrap(), "  cdef\n  ijkl\nmnopqr");
    vt.feed(b"z");
    assert_eq!(vt.plain_text().unwrap(), "  cdef\nz ijkl\nmnopqr");
}

#[test]
fn a_transparent_sixel_keeps_the_text_it_covers() {
    let mut vt = new_vt();
    vt.feed(b"abcdef\x1b[H");
    vt.feed(b"\x1bP0;1q\"1;1;16;16\x1b\\");
    assert_eq!(vt.plain_text().unwrap(), "abcdef");
}

#[test]
fn clearing_the_screen_removes_a_sixel() {
    let mut vt = new_vt();
    vt.feed(&black_sixel(16, 16));
    let key = vt.frame().unwrap().images[0].key;
    vt.feed(b"\x1b[2J");
    assert!(vt.frame().unwrap().images.is_empty());
    assert!(!vt.image_is_stored(&key).unwrap());
}

#[test]
fn with_the_sixel_display_mode_a_sixel_goes_to_the_top_left_and_the_cursor_stays() {
    let mut vt = new_vt();
    vt.feed(b"abcdef\x1b[3;4H\x1b[?80h");
    vt.feed(&black_sixel(16, 16 * 8));
    assert_eq!(vt.cursor_cell().unwrap(), (3, 2));
    let frame = vt.frame().unwrap();
    assert_eq!((frame.images[0].col, frame.images[0].row), (0, 0));
    assert_eq!(vt.scroll_position().unwrap().total, 5);
    assert_eq!(vt.plain_text().unwrap(), "  cdef");
}

#[test]
fn device_attributes_announce_sixel() {
    let mut vt = new_vt();
    vt.feed(b"\x1b[c");
    assert_eq!(vt.take_replies(), b"\x1b[?62;1;4;6;22c");
}

#[test]
fn graphics_attributes_and_pixel_sizes_are_answered_in_the_order_of_the_stream() {
    let mut vt = new_vt();
    // 窓のピクセルの大きさ、色レジスタの数、セルのピクセルの大きさ。
    vt.feed(b"\x1b[14t\x1b[?1;1;0S\x1b[16t\x1b[?2;1;0S");
    assert_eq!(
        String::from_utf8(vt.take_replies()).unwrap(),
        "\x1b[4;80;160t\x1b[?1;0;1024S\x1b[6;16;8t\x1b[?2;0;4096;4096S"
    );
}

#[test]
fn sixels_can_arrive_in_pieces_across_feeds() {
    let mut vt = new_vt();
    for byte in black_sixel(16, 16) {
        vt.feed(&[byte]);
    }
    assert_eq!(vt.frame().unwrap().images.len(), 1);
}

#[test]
fn sixels_do_not_take_the_numbers_of_kitty_images() {
    let mut vt = new_vt();
    graphics(&mut vt, "a=T,f=32,i=1,s=1,v=1,C=1,q=2", "/wAA/w==");
    vt.feed(&black_sixel(16, 16));
    graphics(&mut vt, "a=T,f=32,i=2,s=1,v=1,C=1,q=2", "/wAA/w==");
    vt.feed(&black_sixel(8, 8));
    let frame = vt.frame().unwrap();
    assert_eq!(frame.images.len(), 4);
    let mut ids: Vec<u32> = frame.images.iter().map(|view| view.key.id).collect();
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(ids.len(), 4);
}
