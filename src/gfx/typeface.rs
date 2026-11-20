use ab_glyph::{Font, FontArc};
use std::sync::LazyLock;

static FACE: LazyLock<(FontArc, &'static str)> = LazyLock::new(|| {
    #[cfg(windows)]
    {
        let user = std::env::var_os("LOCALAPPDATA")
            .map(|p| std::path::Path::new(&p).join("Microsoft/Windows/Fonts"));
        let system = std::env::var_os("WINDIR").map(|p| std::path::Path::new(&p).join("Fonts"));
        for (file, name) in [
            ("SFPRODISPLAYMEDIUM.OTF", "SF Pro Display Medium"),
            ("SFPRODISPLAYREGULAR.OTF", "SF Pro Display Regular"),
            ("seguisb.ttf", "Segoe UI Semibold"),
        ] {
            for dir in [user.as_ref(), system.as_ref()].into_iter().flatten() {
                if let Some(font) = std::fs::read(dir.join(file))
                    .ok()
                    .and_then(|b| FontArc::try_from_vec(b).ok())
                {
                    return (font, name);
                }
            }
        }
    }
    (
        FontArc::try_from_slice(include_bytes!("../../fonts/DejaVuSans.ttf"))
            .expect("bundled font"),
        "DejaVu Sans",
    )
});

pub(crate) fn face() -> &'static FontArc {
    &FACE.0
}
#[cfg(windows)]
pub(crate) fn name() -> &'static str {
    FACE.1
}

/// SF Pro Display omits the wide-headed arrows. Keep the lettering in SF
/// and fall back for the symbol alone, including on systems without Segoe.
pub(super) fn arrow(wide: &'static str, simple: &'static str) -> (&'static FontArc, &'static str) {
    let ch = wide.chars().next().expect("arrow constant");
    if face().glyph_id(ch).0 != 0 {
        return (face(), wide);
    }
    #[cfg(windows)]
    {
        static SYMBOLS: LazyLock<Option<FontArc>> = LazyLock::new(|| {
            let dir = std::env::var_os("WINDIR")?;
            FontArc::try_from_vec(
                std::fs::read(std::path::Path::new(&dir).join("Fonts/seguisym.ttf")).ok()?,
            )
            .ok()
        });
        if let Some(font) = SYMBOLS.as_ref().filter(|font| font.glyph_id(ch).0 != 0) {
            return (font, wide);
        }
    }
    (face(), simple)
}
