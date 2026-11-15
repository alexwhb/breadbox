//! Renders the app icon (`assets/icon/breadbox-icon.svg`, the design's logo mark on the macOS
//! icon grid) into an `.iconset` folder for `iconutil` (`scripts/bundle-macos.sh`):
//!
//! ```text
//! cargo run --release -p breadbox --example app_icon -- <out>/Breadbox.iconset
//! iconutil -c icns <out>/Breadbox.iconset -o <out>/Breadbox.icns
//! ```

use std::path::PathBuf;

const SVG: &str = include_str!("../../../assets/icon/breadbox-icon.svg");

/// `icon_<n>x<n>[@2x].png` → pixel size, as `iconutil` expects.
const SIZES: &[(&str, u32)] = &[
    ("icon_16x16.png", 16),
    ("icon_16x16@2x.png", 32),
    ("icon_32x32.png", 32),
    ("icon_32x32@2x.png", 64),
    ("icon_128x128.png", 128),
    ("icon_128x128@2x.png", 256),
    ("icon_256x256.png", 256),
    ("icon_256x256@2x.png", 512),
    ("icon_512x512.png", 512),
    ("icon_512x512@2x.png", 1024),
];

fn main() {
    let out = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("Breadbox.iconset"));
    if let Err(e) = render(&out) {
        eprintln!("app_icon: {e}");
        std::process::exit(1);
    }
}

fn render(out: &std::path::Path) -> Result<(), String> {
    std::fs::create_dir_all(out).map_err(|e| format!("{}: {e}", out.display()))?;
    let opt = resvg::usvg::Options::default();
    let tree = resvg::usvg::Tree::from_str(SVG, &opt).map_err(|e| e.to_string())?;
    let src = tree.size().width();
    for (name, px) in SIZES {
        let mut pix = resvg::tiny_skia::Pixmap::new(*px, *px).ok_or("pixmap")?;
        let s = *px as f32 / src;
        resvg::render(
            &tree,
            resvg::tiny_skia::Transform::from_scale(s, s),
            &mut pix.as_mut(),
        );
        let path = out.join(name);
        let png = pix.encode_png().map_err(|e| e.to_string())?;
        std::fs::write(&path, png).map_err(|e| format!("{}: {e}", path.display()))?;
    }
    println!("{} icon sizes → {}", SIZES.len(), out.display());
    Ok(())
}
