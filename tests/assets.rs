mod common;

use std::fs;
use std::io::{Cursor, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use image::{DynamicImage, ImageFormat, Rgba, RgbaImage};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use termed::app::{App, Focus};
use termed::archive::{self, Index};
use termed::commands::Command;
use termed::document::{Document, Preview};
use termed::editor::Editor;
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

fn project(tag: &str) -> PathBuf {
    static N: AtomicUsize = AtomicUsize::new(0);
    let p = std::env::temp_dir().join(format!(
        "termed-assets-{tag}-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(&p).unwrap();
    p
}
fn png() -> Vec<u8> {
    let mut p = RgbaImage::new(8, 6);
    for (x, y, c) in p.enumerate_pixels_mut() {
        *c = Rgba([
            (x * 30) as u8,
            (y * 40) as u8,
            180,
            if x == 0 { 0 } else { 255 },
        ]);
    }
    let mut out = Cursor::new(Vec::new());
    DynamicImage::ImageRgba8(p)
        .write_to(&mut out, ImageFormat::Png)
        .unwrap();
    out.into_inner()
}
fn zip(path: &Path, entries: &[(&str, &[u8])]) {
    let mut z = ZipWriter::new(fs::File::create(path).unwrap());
    z.set_comment("test archive comment");
    for (name, data) in entries {
        let opt = SimpleFileOptions::default()
            .compression_method(CompressionMethod::Deflated)
            .unix_permissions(0o640);
        if name.ends_with('/') {
            z.add_directory(*name, opt).unwrap();
        } else {
            z.start_file(*name, opt).unwrap();
            z.write_all(data).unwrap();
        }
    }
    z.finish().unwrap();
}
fn member(path: &Path, name: &str) -> Vec<u8> {
    let mut z = ZipArchive::new(fs::File::open(path).unwrap()).unwrap();
    let mut b = Vec::new();
    z.by_name(name).unwrap().read_to_end(&mut b).unwrap();
    b
}
fn app(root: &Path, file: &Path) -> App {
    App::new(
        root.to_path_buf(),
        vec![file.to_path_buf()],
        "Termed Studio",
        false,
    )
}
fn key(a: &mut App, code: KeyCode) {
    a.handle_event(Event::Key(KeyEvent::new(code, KeyModifiers::NONE)));
}

#[test]
fn images_are_decoded_not_lossy_text_and_cannot_be_edited_or_saved_as_text() {
    if common::isolate("images_are_decoded_not_lossy_text_and_cannot_be_edited_or_saved_as_text") {
        return;
    }
    let root = project("png");
    let path = root.join("art.png");
    let bytes = png();
    fs::write(&path, &bytes).unwrap();
    let mut e = Editor::open(path.clone()).unwrap();
    let Some(Preview::Image(image)) = &e.doc.preview else {
        panic!("image preview")
    };
    assert_eq!(image.dimensions(), (8, 6));
    assert_eq!(image.pixels.get_pixel(0, 0)[3], 0);
    assert_eq!(image.original, bytes);
    assert!(e.doc.rope.to_string().is_empty());
    e.insert_text("do not corrupt pixels");
    e.replace_document("no");
    e.doc.insert(0, "also no");
    assert!(!e.doc.is_dirty());
    assert!(e.doc.save().is_err());
    let copy = root.join("export.png");
    e.doc.export_preview(&copy).unwrap();
    assert_eq!(fs::read(&path).unwrap(), bytes);
    assert_eq!(fs::read(copy).unwrap(), bytes);
}

#[test]
fn all_enabled_image_codecs_have_real_decoding_coverage() {
    if common::isolate("all_enabled_image_codecs_have_real_decoding_coverage") {
        return;
    }
    let root = project("formats");
    for (ext, format) in [
        ("png", ImageFormat::Png),
        ("jpg", ImageFormat::Jpeg),
        ("gif", ImageFormat::Gif),
        ("webp", ImageFormat::WebP),
        ("bmp", ImageFormat::Bmp),
        ("ico", ImageFormat::Ico),
    ] {
        let mut out = Cursor::new(Vec::new());
        let image = if format == ImageFormat::Ico {
            DynamicImage::new_rgba8(4, 3)
        } else {
            DynamicImage::new_rgb8(4, 3)
        };
        image.write_to(&mut out, format).unwrap();
        let path = root.join(format!("sample.{ext}"));
        fs::write(&path, out.into_inner()).unwrap();
        let d = Document::open(&path).unwrap();
        assert!(
            matches!(d.preview,Some(Preview::Image(ref p)) if p.dimensions()==(4,3)),
            "{ext}"
        );
    }
}

#[test]
fn jpeg_exif_orientation_is_applied_to_the_preview_only() {
    if common::isolate("jpeg_exif_orientation_is_applied_to_the_preview_only") {
        return;
    }
    let mut encoded = Cursor::new(Vec::new());
    DynamicImage::new_rgb8(2, 3)
        .write_to(&mut encoded, ImageFormat::Jpeg)
        .unwrap();
    // APP1, TIFF little endian, IFD0 orientation = 6 (90 degrees clockwise).
    let exif: &[u8] = b"Exif\0\0II\x2a\0\x08\0\0\0\x01\0\x12\x01\x03\0\x01\0\0\0\x06\0\0\0\0\0\0\0";
    let mut jpeg = encoded.into_inner();
    let mut segment = vec![0xff, 0xe1];
    segment.extend_from_slice(&((exif.len() + 2) as u16).to_be_bytes());
    segment.extend_from_slice(exif);
    jpeg.splice(2..2, segment);
    let p = termed::image_view::ImageView::decode(jpeg.clone()).unwrap();
    assert_eq!(p.dimensions(), (3, 2));
    assert_eq!(p.original, jpeg);
}

#[test]
fn image_canvas_supports_resize_touch_targets_zoom_and_pan() {
    if common::isolate("image_canvas_supports_resize_touch_targets_zoom_and_pan") {
        return;
    }
    let root = project("canvas");
    let file = root.join("art.png");
    fs::write(&file, png()).unwrap();
    let mut a = app(&root, &file);
    for (w, h) in [(140, 42), (80, 24), (46, 30), (28, 18)] {
        let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
        terminal
            .draw(|f| termed::ui::draw(f, &mut a, true))
            .unwrap();
        let b = terminal.backend().buffer();
        let text = b.content.iter().map(|c| c.symbol()).collect::<String>();
        assert!(text.contains("IMAGE PREVIEW"));
        assert!(text.contains("8 × 6"));
        assert!(text.contains('▀'));
        assert!(
            a.layout
                .click_items
                .iter()
                .any(|(r, c)| *c == Command::ImageFit && r.right() <= w && r.bottom() <= h)
        );
        assert!(
            a.layout
                .click_items
                .iter()
                .any(|(_, c)| *c == Command::SaveAs)
        );
    }
    a.focus = Focus::Editor;
    key(&mut a, KeyCode::Char('+'));
    key(&mut a, KeyCode::Right);
    let Some(Preview::Image(p)) = a.editor_mut().unwrap().doc.preview.as_mut() else {
        panic!()
    };
    assert_eq!(p.zoom, 1);
    assert!(p.pan_x > 0);
    let before = p.sample(8, 3).0.get_pixel(0, 0).0;
    p.key(KeyCode::Left);
    let after = p.sample(8, 3).0.get_pixel(0, 0).0;
    assert_ne!(before, after);
    key(&mut a, KeyCode::Char('0'));
    assert!(
        matches!(a.editor().unwrap().doc.preview,Some(Preview::Image(ref p)) if p.zoom==0 && p.pan_x==0)
    );
}

#[test]
fn corrupt_images_and_unrecognized_binary_are_rejected_without_lossy_conversion() {
    if common::isolate(
        "corrupt_images_and_unrecognized_binary_are_rejected_without_lossy_conversion",
    ) {
        return;
    }
    let root = project("corrupt");
    for (name, bytes) in [
        ("bad.png", &b"not an image"[..]),
        ("empty.png", &b""[..]),
        ("binary.dat", &b"hello\0binary"[..]),
    ] {
        let path = root.join(name);
        fs::write(&path, bytes).unwrap();
        assert!(Document::open(&path).is_err(), "{name}");
        assert_eq!(fs::read(path).unwrap(), bytes);
    }
}

#[test]
fn archive_is_both_a_folder_and_an_openable_read_only_file() {
    if common::isolate("archive_is_both_a_folder_and_an_openable_read_only_file") {
        return;
    }
    let root = project("tree");
    let path = root.join("bundle.zip");
    zip(
        &path,
        &[
            ("src/deep/main.rs", b"fn main() {}\n"),
            ("README.md", b"hello"),
            ("empty/", b""),
        ],
    );
    let index = Index::open(&path).unwrap();
    assert_eq!(
        index.children(""),
        vec![
            ("empty".into(), true),
            ("src".into(), true),
            ("README.md".into(), false)
        ]
    );
    let mut a = app(&root, &path);
    assert!(matches!(
        a.editor().unwrap().doc.preview,
        Some(Preview::Archive(_))
    ));
    let pi = a
        .explorer
        .entries
        .iter()
        .position(|e| e.path == path)
        .unwrap();
    assert!(a.explorer.entries[pi].is_archive);
    a.explorer.selected = pi;
    a.focus = Focus::Explorer;
    key(&mut a, KeyCode::Right);
    assert!(
        a.explorer
            .entries
            .iter()
            .any(|e| e.path == path.join("src") && e.is_dir)
    );
    a.explorer.reveal(&path.join("src/deep/main.rs"));
    assert!(
        a.explorer
            .entries
            .iter()
            .any(|e| e.path == path.join("src/deep/main.rs") && !e.is_dir)
    );
    a.explorer.selected = 0; // find root again after expanding descendants
    a.explorer.selected = a
        .explorer
        .entries
        .iter()
        .position(|e| e.path == path)
        .unwrap();
    a.handle_event(Event::Key(KeyEvent::new(
        KeyCode::Enter,
        KeyModifiers::SHIFT,
    )));
    assert!(a.editor().unwrap().doc.is_read_only());
    let original = fs::read(&path).unwrap();
    let copy = root.join("copy.zip");
    a.editor().unwrap().doc.export_preview(&copy).unwrap();
    assert_eq!(fs::read(copy).unwrap(), original);
    zip(&path, &[("changed.txt", b"changed archive")]);
    a.open_file(&path);
    assert!(
        a.editor()
            .unwrap()
            .doc
            .rope
            .to_string()
            .contains("changed.txt")
    );
    a.editor()
        .unwrap()
        .doc
        .export_preview(&root.join("refreshed.zip"))
        .unwrap();
}

#[test]
fn zip_members_edit_save_and_undo_while_preserving_other_files_and_comments() {
    if common::isolate("zip_members_edit_save_and_undo_while_preserving_other_files_and_comments") {
        return;
    }
    let root = project("edit");
    let path = root.join("project.zip");
    zip(
        &path,
        &[
            ("src/main.rs", b"fn main() {}\r\n"),
            ("keep.bin", b"\0\x01\xff"),
        ],
    );
    let mut e = Editor::open(path.join("src/main.rs")).unwrap();
    assert_eq!(e.doc.rope.to_string(), "fn main() {}\n");
    e.replace_document("fn main() { println!(\"hi\"); }\n");
    e.doc.save().unwrap();
    assert_eq!(
        member(&path, "src/main.rs"),
        b"fn main() { println!(\"hi\"); }\r\n"
    );
    assert_eq!(member(&path, "keep.bin"), b"\0\x01\xff");
    let z = ZipArchive::new(fs::File::open(&path).unwrap()).unwrap();
    assert_eq!(z.comment(), b"test archive comment");
    drop(z);
    let mut z = ZipArchive::new(fs::File::open(&path).unwrap()).unwrap();
    assert_eq!(
        z.by_name("keep.bin").unwrap().unix_mode().unwrap() & 0o777,
        0o640
    );
    drop(z);
    e.undo();
    assert_eq!(e.doc.rope.to_string(), "fn main() {}\n");
    e.doc.save().unwrap();
    assert_eq!(member(&path, "src/main.rs"), b"fn main() {}\r\n");
}

#[test]
fn different_zip_members_can_save_concurrently_but_same_member_conflicts_are_kept() {
    if common::isolate(
        "different_zip_members_can_save_concurrently_but_same_member_conflicts_are_kept",
    ) {
        return;
    }
    let root = project("conflict");
    let path = root.join("project.zip");
    zip(&path, &[("a.txt", b"A"), ("b.txt", b"B")]);
    let mut a = Editor::open(path.join("a.txt")).unwrap();
    let mut b = Editor::open(path.join("b.txt")).unwrap();
    a.replace_document("A changed");
    b.replace_document("B changed");
    a.doc.save().unwrap();
    b.doc.save().unwrap();
    assert_eq!(member(&path, "a.txt"), b"A changed");
    assert_eq!(member(&path, "b.txt"), b"B changed");
    a.replace_document("unsaved");
    zip(&path, &[("a.txt", b"external"), ("b.txt", b"B changed")]);
    assert!(a.doc.save().is_err());
    assert!(a.doc.is_dirty());
    assert_eq!(a.doc.rope.to_string(), "unsaved");
    assert_eq!(member(&path, "a.txt"), b"external");
    assert!(
        fs::read_dir(root)
            .unwrap()
            .flatten()
            .all(|e| !e.file_name().to_string_lossy().ends_with(".tmp"))
    );
}

#[test]
fn new_text_members_and_save_as_exports_use_the_correct_container() {
    if common::isolate("new_text_members_and_save_as_exports_use_the_correct_container") {
        return;
    }
    let root = project("new-member");
    let path = root.join("project.zip");
    zip(&path, &[("a.txt", b"old")]);
    let mut d = Document::from_text("new content", None);
    d.save_as(&path.join("new/folder/new.txt")).unwrap();
    assert_eq!(member(&path, "new/folder/new.txt"), b"new content");
    assert_eq!(member(&path, "a.txt"), b"old");
    let export = root.join("export.txt");
    d.save_as(&export).unwrap();
    assert_eq!(fs::read(export).unwrap(), b"new content");
    let image = png();
    zip(&path, &[("images/art.png", &image)]);
    let d = Document::open(&path.join("images/art.png")).unwrap();
    let exported = root.join("export.png");
    d.export_preview(&exported).unwrap();
    assert_eq!(fs::read(exported).unwrap(), image);
    assert!(
        d.export_preview(&path).is_err(),
        "never replace the containing ZIP with a PNG"
    );
}

#[test]
fn disappearing_zip_never_turns_into_an_ordinary_directory_on_save() {
    if common::isolate("disappearing_zip_never_turns_into_an_ordinary_directory_on_save") {
        return;
    }
    let root = project("missing");
    let path = root.join("project.zip");
    zip(&path, &[("a.txt", b"old")]);
    let mut e = Editor::open(path.join("a.txt")).unwrap();
    e.replace_document("unsaved");
    fs::remove_file(&path).unwrap();
    assert!(e.doc.save().is_err());
    assert!(!path.exists());
    assert!(e.doc.is_dirty());
    let mut new = Document::from_text("new text", None);
    assert!(new.save_as(&path.join("new.txt")).is_err());
    assert!(!path.exists());
}

#[test]
fn unsafe_zip_paths_and_file_folder_conflicts_are_refused() {
    if common::isolate("unsafe_zip_paths_and_file_folder_conflicts_are_refused") {
        return;
    }
    let root = project("unsafe");
    let path = root.join("bad.zip");
    for name in [
        "../escape",
        "/absolute",
        "C:/drive",
        "a\\b",
        "a//b",
        "dir//",
        "a/./b",
        "control\nname",
    ] {
        zip(&path, &[(name, b"no")]);
        assert!(Index::open(&path).is_err(), "{name}");
    }
    zip(&path, &[("file", b"no"), ("file/child", b"no")]);
    assert!(Index::open(&path).is_err());
    assert!(!root.join("escape").exists());
}

#[test]
fn zip_links_and_encryption_are_not_damaged_and_dot_zip_directories_work() {
    if common::isolate("zip_links_and_encryption_are_not_damaged_and_dot_zip_directories_work") {
        return;
    }
    let root = project("links");
    let path = root.join("links.zip");
    let mut z = ZipWriter::new(fs::File::create(&path).unwrap());
    z.add_symlink("link", "../secret.txt", SimpleFileOptions::default())
        .unwrap();
    z.finish().unwrap();
    assert!(Document::open(&path.join("link")).is_err());
    let original = fs::read(&path).unwrap();
    let mut d = Document::from_text("new text", None);
    assert!(d.save_as(&path.join("notes.txt")).is_err());
    assert_eq!(fs::read(&path).unwrap(), original);
    zip(&path, &[("a.txt", b"plain"), ("encrypted.txt", b"opaque")]);
    let mut z = ZipArchive::new(fs::File::open(&path).unwrap()).unwrap();
    let file = z.by_name("encrypted.txt").unwrap();
    let (local, central) = (
        file.header_start() as usize,
        file.central_header_start() as usize,
    );
    drop(file);
    drop(z);
    let mut bytes = fs::read(&path).unwrap();
    bytes[local + 6] |= 1;
    bytes[central + 8] |= 1;
    fs::write(&path, &bytes).unwrap();
    let mut e = Editor::open(path.join("a.txt")).unwrap();
    e.replace_document("unsaved");
    assert!(e.doc.save().is_err());
    assert!(e.doc.is_dirty());
    assert_eq!(fs::read(&path).unwrap(), bytes);
    #[cfg(unix)]
    {
        zip(&path, &[("a.txt", b"before")]);
        let alias = root.join("alias.zip");
        std::os::unix::fs::symlink(&path, &alias).unwrap();
        let mut e = Editor::open(alias.join("a.txt")).unwrap();
        e.replace_document("after");
        e.doc.save().unwrap();
        assert_eq!(member(&path, "a.txt"), b"after");
        assert!(
            fs::symlink_metadata(alias)
                .unwrap()
                .file_type()
                .is_symlink()
        );
    }
    let folder = root.join("ordinary.zip");
    fs::create_dir(&folder).unwrap();
    fs::write(folder.join("a.txt"), "normal").unwrap();
    assert!(archive::member_path(&folder.join("a.txt")).is_none());
    assert_eq!(
        Document::open(&folder.join("a.txt"))
            .unwrap()
            .rope
            .to_string(),
        "normal"
    );
}

#[test]
fn studio_theme_keeps_primary_and_secondary_chrome_text_readable() {
    if common::isolate("studio_theme_keeps_primary_and_secondary_chrome_text_readable") {
        return;
    }
    let t = termed::theme::load("Termed Studio").unwrap();
    let p = t.palette;
    let luminance = |c| {
        let (r, g, b) = termed::theme::rgb(c);
        let f = |v: u8| {
            let v = f64::from(v) / 255.0;
            if v <= 0.04045 {
                v / 12.92
            } else {
                ((v + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * f(r) + 0.7152 * f(g) + 0.0722 * f(b)
    };
    for (fg, bg) in [
        (p.fg, p.bg),
        (p.sidebar_fg, p.sidebar_bg),
        (p.sidebar_dim, p.sidebar_bg),
        (p.sidebar_dim, p.bg),
        (p.fg, p.input_bg),
        (p.sidebar_fg, p.sidebar_selected),
        (p.popup_fg, p.popup_bg),
        (p.popup_dim, p.popup_bg),
        (p.status_fg, p.status_bg),
        (p.accent_fg, p.accent),
    ] {
        let (a, b) = (luminance(fg), luminance(bg));
        assert!((a.max(b) + 0.05) / (a.min(b) + 0.05) >= 4.5);
    }
}

#[test]
fn resource_limits_reject_oversized_images_members_and_archive_indexes() {
    if common::isolate("resource_limits_reject_oversized_images_members_and_archive_indexes") {
        return;
    }
    let root = project("limits");
    let mut image = png();
    image[16..20].copy_from_slice(&4097u32.to_be_bytes());
    image[20..24].copy_from_slice(&4096u32.to_be_bytes());
    let mut crc = !0u32;
    for byte in &image[12..29] {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0xedb88320u32 & 0u32.wrapping_sub(crc & 1));
        }
    }
    image[29..33].copy_from_slice(&(!crc).to_be_bytes());
    let error = termed::image_view::ImageView::decode(image)
        .err()
        .expect("oversized dimensions");
    assert!(format!("{error:#}").contains("16 megapixel"), "{error:#}");
    let huge = root.join("huge.zip");
    fs::File::create(&huge)
        .unwrap()
        .set_len(archive::MAX_ARCHIVE_BYTES + 1)
        .unwrap();
    assert!(format!("{:#}", Index::open(&huge).unwrap_err()).contains("256 MB"));
    fs::remove_file(huge).unwrap();
    let path = root.join("project.zip");
    zip(&path, &[("a.txt", b"small")]);
    let mut bytes = fs::read(&path).unwrap();
    let central = bytes.windows(4).position(|v| v == b"PK\x01\x02").unwrap();
    bytes[central + 24..central + 28]
        .copy_from_slice(&((archive::MAX_MEMBER_BYTES + 1) as u32).to_le_bytes());
    fs::write(&path, &bytes).unwrap();
    assert!(format!("{:#}", Document::open(&path.join("a.txt")).err().unwrap()).contains("32 MB"));
    let end = bytes.windows(4).rposition(|v| v == b"PK\x05\x06").unwrap();
    for at in [end + 8, end + 10] {
        bytes[at..at + 2].copy_from_slice(&10001u16.to_le_bytes());
    }
    fs::write(&path, bytes).unwrap();
    assert!(format!("{:#}", Index::open(&path).unwrap_err()).contains("10,000"));
    // A lying ZIP64 footer must also be rejected before the dependency allocates its index.
    let mut bytes = vec![0u8; 98];
    bytes[..4].copy_from_slice(b"PK\x06\x06");
    bytes[4..12].copy_from_slice(&44u64.to_le_bytes());
    bytes[24..32].copy_from_slice(&10001u64.to_le_bytes());
    bytes[32..40].copy_from_slice(&10001u64.to_le_bytes());
    bytes[56..60].copy_from_slice(b"PK\x06\x07");
    bytes[76..80].copy_from_slice(b"PK\x05\x06");
    let mut valid = bytes.clone();
    valid[12..14].copy_from_slice(&45u16.to_le_bytes());
    valid[14..16].copy_from_slice(&45u16.to_le_bytes());
    valid[24..40].fill(0);
    valid[72..76].copy_from_slice(&1u32.to_le_bytes());
    valid[84..88].fill(255);
    valid[88..96].fill(255);
    fs::write(&path, valid).unwrap();
    assert!(Index::open(&path).unwrap().entries.is_empty());
    fs::write(&path, bytes).unwrap();
    assert!(format!("{:#}", Index::open(&path).unwrap_err()).contains("10,000"));
}

#[test]
fn corrupt_zip_member_crc_is_reported_without_returning_partial_text() {
    if common::isolate("corrupt_zip_member_crc_is_reported_without_returning_partial_text") {
        return;
    }
    let root = project("crc");
    let path = root.join("corrupt.zip");
    let mut z = ZipWriter::new(fs::File::create(&path).unwrap());
    z.start_file(
        "a.txt",
        SimpleFileOptions::default().compression_method(CompressionMethod::Stored),
    )
    .unwrap();
    z.write_all(b"integrity checked").unwrap();
    z.finish().unwrap();
    let mut z = ZipArchive::new(fs::File::open(&path).unwrap()).unwrap();
    let start = z.by_name("a.txt").unwrap().data_start() as usize;
    drop(z);
    let mut bytes = fs::read(&path).unwrap();
    bytes[start] ^= 1;
    fs::write(&path, &bytes).unwrap();
    let error = Document::open(&path.join("a.txt"))
        .err()
        .expect("CRC must fail");
    assert!(format!("{error:#}").contains("CRC"));
    assert_eq!(fs::read(path).unwrap(), bytes);
}

#[test]
fn exports_and_save_as_never_overwrite_their_source_container_or_binary_alias() {
    if common::isolate("exports_and_save_as_never_overwrite_their_source_container_or_binary_alias")
    {
        return;
    }
    let root = project("aliases");
    let path = root.join("project.zip");
    let pixels = png();
    zip(&path, &[("art.png", &pixels), ("a.txt", b"original")]);
    let original = fs::read(&path).unwrap();
    let image = Document::open(&path.join("art.png")).unwrap();
    let mut text = Editor::open(path.join("a.txt")).unwrap();
    text.replace_document("unsaved");
    assert!(text.doc.save_as(&path).is_err());
    assert_eq!(text.doc.path, Some(path.join("a.txt")));
    assert!(text.doc.is_dirty());
    let alias = root.join("alias.dat");
    fs::hard_link(&path, &alias).unwrap();
    assert!(text.doc.save_as(&alias).is_err());
    assert_eq!(fs::read(&alias).unwrap(), original);
    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;
        assert!(image.export_preview(&alias).is_err());
        let link = root.join("symlink.dat");
        symlink(&path, &link).unwrap();
        assert!(image.export_preview(&link).is_err());
    }
    assert_eq!(fs::read(&path).unwrap(), original);
    let invalid = root.join("no-parent").join("notes.txt");
    fs::write(root.join("no-parent"), b"file").unwrap();
    assert!(text.doc.save_as(&invalid).is_err());
    assert_eq!(text.doc.path, Some(path.join("a.txt")));
    assert!(text.doc.is_dirty());
}

#[test]
fn svg_remains_editable_source_instead_of_a_broken_binary_preview() {
    if common::isolate("svg_remains_editable_source_instead_of_a_broken_binary_preview") {
        return;
    }
    let root = project("svg");
    let path = root.join("vector.svg");
    let svg = "<svg xmlns=\"http://www.w3.org/2000/svg\"><circle r=\"4\"/></svg>";
    fs::write(&path, svg).unwrap();
    let mut e = Editor::open(path.clone()).unwrap();
    assert!(!e.doc.is_read_only());
    assert_eq!(e.doc.rope.to_string(), svg);
    e.replace_document("<svg/>");
    e.doc.save().unwrap();
    assert_eq!(fs::read_to_string(path).unwrap(), "<svg/>");
}

#[test]
fn read_only_previews_block_every_text_command_and_do_not_pollute_project_search() {
    if common::isolate(
        "read_only_previews_block_every_text_command_and_do_not_pollute_project_search",
    ) {
        return;
    }
    let root = project("read-only");
    let path = root.join("project.zip");
    zip(&path, &[("needle-only-in-filename.txt", b"original")]);
    let mut a = app(&root, &path);
    let original = a.editor().unwrap().doc.rope.to_string();
    let e = a.editor_mut().unwrap();
    e.type_char('(');
    e.tab_key();
    e.insert_lines_above("no");
    e.indent_lines(true);
    e.toggle_comment();
    e.move_lines(true);
    e.duplicate_lines(true);
    e.delete_lines();
    e.insert_line(true);
    e.delete_word(true);
    e.replace_all(&[(0, 2)], "no");
    e.box_to(0, 0, 1, 2);
    e.box_type("no");
    e.box_backspace();
    e.box_delete();
    e.box_paste("no");
    e.undo();
    e.redo();
    assert_eq!(e.doc.rope.to_string(), original);
    assert!(!e.doc.is_dirty());
    a.search.visible = true;
    a.search.query = "needle-only-in-filename".into();
    a.update_search();
    assert!(a.search.results.files.is_empty());
    a.open_file(&path.join("needle-only-in-filename.txt"));
    a.editor_mut()
        .unwrap()
        .replace_document("live archive text needle");
    a.search.query = "live archive text needle".into();
    a.update_search();
    assert_eq!(a.search.results.files.len(), 1);
    assert_eq!(
        a.search.results.files[0].path,
        path.join("needle-only-in-filename.txt")
    );
}
