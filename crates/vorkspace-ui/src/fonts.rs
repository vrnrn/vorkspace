/// GPUI resolves this family to the platform system UI font.
pub const UI: &str = ".SystemUIFont";
pub const FIXED_FONT: &str = "Menlo";
/// Code, keyboard hints and numeric readouts in the chat and chrome. The
/// terminal and code editor keep `FIXED_FONT`. See `fluid::MONO`.
pub const MONO: &str = crate::fluid::MONO;
