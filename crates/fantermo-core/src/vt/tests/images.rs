use super::*;

#[test]
fn a_png_image_is_placed_at_the_cursor_above_the_text() {
    let mut vt = new_vt();
    vt.feed(b"\x1b[2;3H");
    graphics(&mut vt, "a=T,f=100,i=7,q=2", RED_PNG);
    let frame = vt.frame().unwrap();
    let [view] = frame.images.as_slice() else {
        panic!("{:?}", frame.images);
    };
    assert_eq!(view.key.id, 7);
    assert_eq!(view.layer, ImageLayer::AboveText);
    assert_eq!((view.col, view.row), (2, 1));
    assert_eq!((view.width, view.height), (1, 1));
    let pixels = vt.image_pixels(&view.key).unwrap().unwrap();
    assert_eq!((pixels.width, pixels.height), (1, 1));
    assert_eq!(pixels.rgba.as_ref(), [255, 0, 0, 255]);
}

#[test]
fn raw_rgb_is_converted_to_rgba_and_spans_rows() {
    let mut vt = new_vt();
    // 8×16 のセルで 10×20 ピクセルは 2 桁・2 行にまたがる。
    let (width, height) = (10u32, 20u32);
    let rgb: Vec<u8> = (0..width * height)
        .flat_map(|pixel| [(pixel % 256) as u8, 1, 2])
        .collect();
    let control = format!("a=T,f=24,s={width},v={height},i=2,q=2");
    graphics(&mut vt, &control, &base64(&rgb));
    let frame = vt.frame().unwrap();
    let view = frame.images[0];
    assert_eq!((view.width, view.height), (width, height));
    assert_eq!((view.source_width, view.source_height), (width, height));
    // 画像の下の行へカーソルが進む。
    assert_eq!(vt.cursor_cell().unwrap().1, 1);
    let pixels = vt.image_pixels(&view.key).unwrap().unwrap();
    assert_eq!(&pixels.rgba[..8], [0, 1, 2, 255, 1, 1, 2, 255]);
    assert_eq!(pixels.rgba.len(), (width * height * 4) as usize);
}

#[test]
fn gray_pixels_are_converted_to_rgba() {
    assert_eq!(
        rgba_pixels(ImageFormat::Gray, &[9], 1, 1).unwrap().rgba,
        [9, 9, 9, 255].as_slice()
    );
    assert_eq!(
        rgba_pixels(ImageFormat::GrayAlpha, &[9, 3], 1, 1)
            .unwrap()
            .rgba,
        [9, 9, 9, 3].as_slice()
    );
    // バイト数が足りなければ読まない。
    assert_eq!(rgba_pixels(ImageFormat::Rgb, &[1, 2], 1, 1), None);
}

#[test]
fn chunks_become_one_image() {
    let mut vt = new_vt();
    let rgba = base64(&[10, 20, 30, 255, 40, 50, 60, 255, 70, 80, 90, 255]);
    let (first, rest) = rgba.split_at(4);
    graphics(&mut vt, "a=T,f=32,s=3,v=1,i=3,q=2,m=1", first);
    assert!(vt.frame().unwrap().images.is_empty());
    graphics(&mut vt, "m=0", rest);
    let frame = vt.frame().unwrap();
    let pixels = vt.image_pixels(&frame.images[0].key).unwrap().unwrap();
    assert_eq!(
        pixels.rgba.as_ref(),
        [10, 20, 30, 255, 40, 50, 60, 255, 70, 80, 90, 255]
    );
}

#[test]
fn deleting_removes_the_image_from_the_frame_and_the_storage() {
    let mut vt = new_vt();
    graphics(&mut vt, "a=T,f=100,i=4,q=2", RED_PNG);
    let key = vt.frame().unwrap().images[0].key;
    assert!(vt.image_is_stored(&key).unwrap());
    vt.feed(b"\x1b_Ga=d,d=I,i=4,q=2\x1b\\");
    assert!(vt.frame().unwrap().images.is_empty());
    assert!(!vt.image_is_stored(&key).unwrap());
    assert_eq!(vt.image_pixels(&key).unwrap(), None);
}

#[test]
fn sending_again_changes_the_generation() {
    let mut vt = new_vt();
    graphics(&mut vt, "a=T,f=100,i=5,q=2", RED_PNG);
    let old = vt.frame().unwrap().images[0].key;
    graphics(&mut vt, "a=T,f=100,i=5,q=2", RED_PNG);
    let new = vt.frame().unwrap().images[0].key;
    assert_ne!(old.generation, new.generation);
    assert!(!vt.image_is_stored(&old).unwrap());
    assert!(vt.image_is_stored(&new).unwrap());
}

#[test]
fn queries_are_answered() {
    let mut vt = new_vt();
    graphics(&mut vt, "i=31,s=1,v=1,a=q,t=d,f=24", "AAAA");
    assert_eq!(vt.take_replies(), b"\x1b_Gi=31;OK\x1b\\");
    // 確認するだけで、保管も画面も変わらない。
    assert!(vt.frame().unwrap().images.is_empty());
}

#[test]
fn files_cannot_be_read_as_images() {
    let mut vt = new_vt();
    let path = base64(br"C:\Windows\win.ini");
    graphics(&mut vt, "i=32,s=1,v=1,a=q,t=f,f=24", &path);
    let replies = vt.take_replies();
    let reply = String::from_utf8_lossy(&replies);
    assert!(reply.starts_with("\x1b_Gi=32;"), "{reply:?}");
    assert!(!reply.contains(";OK"), "{reply:?}");
}

#[test]
fn images_scroll_with_the_text_and_into_the_scrollback() {
    let mut vt = new_vt();
    graphics(&mut vt, "a=T,f=100,i=6,q=2", RED_PNG);
    assert_eq!(vt.frame().unwrap().images[0].row, 0);
    vt.feed(b"\r\n\r\n\r\n\r\n\r\n");
    // 上へ流れて、表示している範囲から出た。
    let scrolled = vt.frame().unwrap().images;
    assert!(scrolled.is_empty(), "{scrolled:?}");
    vt.scroll_to_row(0);
    assert_eq!(vt.frame().unwrap().images[0].row, 0);
}

#[test]
fn each_screen_has_its_own_images() {
    let mut vt = new_vt();
    graphics(&mut vt, "a=T,f=100,i=8,q=2", RED_PNG);
    let key = vt.frame().unwrap().images[0].key;
    vt.feed(b"\x1b[?1049h");
    assert!(vt.frame().unwrap().images.is_empty());
    assert!(!vt.image_is_stored(&key).unwrap());
    vt.feed(b"\x1b[?1049l");
    assert_eq!(vt.frame().unwrap().images[0].key, key);
    assert!(vt.image_is_stored(&key).unwrap());
}

#[test]
fn layers_and_z_decide_the_order() {
    let mut vt = new_vt();
    let z_below_background = i32::MIN;
    for (id, z) in [(1, 5), (2, -1), (3, z_below_background), (4, 0), (5, 0)] {
        graphics(&mut vt, &format!("a=T,f=100,i={id},z={z},q=2"), RED_PNG);
    }
    let frame = vt.frame().unwrap();
    let order: Vec<(u32, ImageLayer)> = frame
        .images
        .iter()
        .map(|view| (view.key.id, view.layer))
        .collect();
    assert_eq!(
        order,
        [
            (3, ImageLayer::BelowBackground),
            (2, ImageLayer::BelowText),
            (4, ImageLayer::AboveText),
            (5, ImageLayer::AboveText),
            (1, ImageLayer::AboveText),
        ]
    );
}

#[test]
fn placeholder_cells_show_the_image_instead_of_the_character() {
    let mut vt = new_vt();
    // 画像 3 の仮想の配置（2 桁・1 行）と、それを見せる 2 つのセル。
    graphics(&mut vt, "a=T,U=1,f=32,i=3,s=1,v=1,c=2,r=1,q=2", "AP8A/w==");
    vt.feed("\x1b[38;5;3m\u{10EEEE}\u{0305}\u{0305}\u{10EEEE}\u{0305}\u{030D}\x1b[39m".as_bytes());
    let frame = vt.frame().unwrap();
    let [view] = frame.images.as_slice() else {
        panic!("{:?}", frame.images);
    };
    assert_eq!(view.key.id, 3);
    assert_eq!(view.layer, ImageLayer::BelowText);
    assert_eq!((view.width, view.height), (16, 16));
    assert!(frame.cells.iter().all(|cell| cell.text.is_empty()));
}

#[test]
fn animations_report_the_next_frame() {
    let mut vt = new_vt();
    graphics(&mut vt, "a=T,f=32,i=2,s=1,v=1,q=2", "/wAA/w==");
    assert_eq!(vt.tick_image_animations(Duration::ZERO).unwrap(), None);
    let first = vt.frame().unwrap().images[0].key;
    graphics(&mut vt, "a=f,f=32,i=2,s=1,v=1,z=40,q=2", "AAD//w==");
    vt.feed(b"\x1b_Ga=a,i=2,r=1,z=30,q=2\x1b\\\x1b_Ga=a,i=2,s=3,q=2\x1b\\");
    assert_eq!(
        vt.tick_image_animations(Duration::ZERO).unwrap(),
        Some(Duration::from_millis(30))
    );
    assert_eq!(
        vt.tick_image_animations(Duration::from_millis(30)).unwrap(),
        Some(Duration::from_millis(40))
    );
    let second = vt.frame().unwrap().images[0].key;
    assert_ne!(first.generation, second.generation);
    let pixels = vt.image_pixels(&second).unwrap().unwrap();
    assert_eq!(pixels.rgba.as_ref(), [0, 0, 255, 255]);
}
