//! StitchForge in the browser: the same app and engine as the desktop
//! build, compiled to WebAssembly.

#[path = "../../src/bin/stitchforge-app.rs"]
#[allow(dead_code)]
mod app;

pub use app::web::WebHandle;

/// Open-licensed fonts shipped with the web app (SIL OFL): browsers don't
/// expose installed fonts, so these are always there, and any .ttf/.otf can
/// be added with "Load font file…".
pub(crate) const BUNDLED_FONTS: &[&[u8]] = &[
    include_bytes!("../fonts/LiberationSans-Regular.ttf"),
    include_bytes!("../fonts/LiberationSerif-Regular.ttf"),
    include_bytes!("../fonts/Poppins-Regular.ttf"),
    include_bytes!("../fonts/Lora-Variable.ttf"),
    include_bytes!("../fonts/Inter-Regular.otf"),
];
