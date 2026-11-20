//! Stub SDL2 bindings used when the `sdl2` feature is disabled (e.g. the
//! native armv7 build, which uses the TV's own EGL/GLES2 instead).

#![allow(dead_code)]

pub const SDL_EVENT_SIZE: usize = 56;
pub const SDL_QUIT: u32 = 0x100;
pub const SDL_KEYDOWN: u32 = 0x300;
pub const SDLK_ESCAPE: i32 = 27;
pub const SDLK_q: i32 = 113;

pub struct Window;
pub struct Renderer;
pub struct Texture;

pub fn init() -> Result<(), String> {
    Err("SDL2 support is not compiled in".into())
}
pub fn quit() {}
pub fn create_window(_t: &str, _w: i32, _h: i32, _f: bool) -> Result<Window, String> {
    Err("SDL2 support is not compiled in".into())
}
pub fn create_renderer(_w: &Window) -> Result<Renderer, String> {
    Err("SDL2 support is not compiled in".into())
}
pub fn create_texture(_r: &Renderer, _w: i32, _h: i32) -> Result<Texture, String> {
    Err("SDL2 support is not compiled in".into())
}
pub fn update_texture(_t: &Texture, _p: &[u8], _w: i32, _h: i32) -> Result<(), String> {
    Err("SDL2 support is not compiled in".into())
}
pub fn present(_r: &Renderer, _t: &Texture, _w: &Window) {}
pub fn poll_event(_buf: &mut [u8; SDL_EVENT_SIZE]) -> Option<(u32, i32)> {
    None
}
pub fn destroy(_w: &Window, _r: &Renderer, _t: &Texture) {}
