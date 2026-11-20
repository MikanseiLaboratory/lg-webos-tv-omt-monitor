//! Minimal hand-written SDL2 bindings.
//!
//! The webOS bridge ships a 64-bit SDL2 (`libSDL2-2.0.so.0`) but no headers, and
//! `sdl2-sys`'s bindgen path is painful to cross-compile. We only need a dozen
//! entry points, so they are declared here directly.

#![allow(non_camel_case_types, dead_code)]

use core::ffi::{c_char, c_int, c_void};

pub const SDL_INIT_VIDEO: u32 = 0x0000_0020;
pub const SDL_INIT_EVENTS: u32 = 0x0000_4000;

pub const SDL_WINDOW_FULLSCREEN_DESKTOP: u32 = 0x0000_1001;

pub const SDL_PIXELFORMAT_ARGB8888: u32 = 0x1636_2004;

pub const SDL_TEXTUREACCESS_STREAMING: c_int = 1;

pub const SDL_QUIT: u32 = 0x0000_0100;
pub const SDL_KEYDOWN: u32 = 0x0000_0300;

pub const SDLK_ESCAPE: i32 = 27;
pub const SDLK_q: i32 = 113;

/// SDL_Rect
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct SDL_Rect {
    pub x: c_int,
    pub y: c_int,
    pub w: c_int,
    pub h: c_int,
}

/// `sizeof(SDL_Event)` on 64-bit platforms.
pub const SDL_EVENT_SIZE: usize = 56;

pub struct Window(*mut c_void);
pub struct Renderer(*mut c_void);
pub struct Texture(*mut c_void);

unsafe impl Send for Window {}
unsafe impl Send for Renderer {}
unsafe impl Send for Texture {}

impl Window {
    pub fn as_ptr(&self) -> *mut c_void {
        self.0
    }
}

impl Renderer {
    pub fn as_ptr(&self) -> *mut c_void {
        self.0
    }
}

impl Texture {
    pub fn as_ptr(&self) -> *mut c_void {
        self.0
    }
}

unsafe extern "C" {
    fn SDL_Init(flags: u32) -> c_int;
    fn SDL_Quit();
    fn SDL_GetError() -> *const c_char;
    fn SDL_CreateWindow(
        title: *const c_char,
        x: c_int,
        y: c_int,
        w: c_int,
        h: c_int,
        flags: u32,
    ) -> *mut c_void;
    fn SDL_DestroyWindow(window: *mut c_void);
    fn SDL_CreateRenderer(
        window: *mut c_void,
        index: c_int,
        flags: u32,
    ) -> *mut c_void;
    fn SDL_DestroyRenderer(renderer: *mut c_void);
    fn SDL_CreateTexture(
        renderer: *mut c_void,
        format: u32,
        access: c_int,
        w: c_int,
        h: c_int,
    ) -> *mut c_void;
    fn SDL_DestroyTexture(texture: *mut c_void);
    fn SDL_UpdateTexture(
        texture: *mut c_void,
        rect: *const SDL_Rect,
        pixels: *const c_void,
        pitch: c_int,
    ) -> c_int;
    fn SDL_RenderClear(renderer: *mut c_void) -> c_int;
    fn SDL_RenderCopy(
        renderer: *mut c_void,
        texture: *mut c_void,
        srcrect: *const SDL_Rect,
        dstrect: *const SDL_Rect,
    ) -> c_int;
    fn SDL_RenderPresent(renderer: *mut c_void);
    fn SDL_PollEvent(event: *mut c_void) -> c_int;
    fn SDL_Delay(ms: u32);
    fn SDL_GetTicks() -> u32;
    fn SDL_GetWindowSize(window: *mut c_void, w: *mut c_int, h: *mut c_int);
    fn SDL_SetWindowTitle(window: *mut c_void, title: *const c_char);
}

fn err() -> String {
    unsafe {
        let p = SDL_GetError();
        if p.is_null() {
            "unknown SDL error".to_string()
        } else {
            core::ffi::CStr::from_ptr(p).to_string_lossy().into_owned()
        }
    }
}

pub fn init() -> Result<(), String> {
    let r = unsafe { SDL_Init(SDL_INIT_VIDEO | SDL_INIT_EVENTS) };
    if r != 0 {
        return Err(format!("SDL_Init failed: {}", err()));
    }
    Ok(())
}

pub fn quit() {
    unsafe { SDL_Quit() }
}

pub fn create_window(title: &str, w: i32, h: i32, fullscreen: bool) -> Result<Window, String> {
    let mut ctitle = title.as_bytes().to_vec();
    ctitle.push(0);
    let flags = if fullscreen { SDL_WINDOW_FULLSCREEN_DESKTOP } else { 0 };
    let p = unsafe {
        SDL_CreateWindow(
            ctitle.as_ptr() as *const c_char,
            0,
            0,
            w,
            h,
            flags,
        )
    };
    if p.is_null() {
        Err(format!("SDL_CreateWindow failed: {}", err()))
    } else {
        Ok(Window(p))
    }
}

pub fn create_renderer(window: &Window) -> Result<Renderer, String> {
    // index -1 = first renderer that supports the requested flags
    let p = unsafe { SDL_CreateRenderer(window.as_ptr(), -1, 0) };
    if p.is_null() {
        Err(format!("SDL_CreateRenderer failed: {}", err()))
    } else {
        Ok(Renderer(p))
    }
}

pub fn create_texture(
    renderer: &Renderer,
    w: i32,
    h: i32,
) -> Result<Texture, String> {
    let p = unsafe {
        SDL_CreateTexture(
            renderer.as_ptr(),
            SDL_PIXELFORMAT_ARGB8888,
            SDL_TEXTUREACCESS_STREAMING,
            w,
            h,
        )
    };
    if p.is_null() {
        Err(format!("SDL_CreateTexture failed: {}", err()))
    } else {
        Ok(Texture(p))
    }
}

pub fn update_texture(texture: &Texture, pixels: &[u8], w: i32, h: i32) -> Result<(), String> {
    let r = unsafe {
        SDL_UpdateTexture(
            texture.as_ptr(),
            core::ptr::null(),
            pixels.as_ptr() as *const c_void,
            w * 4,
        )
    };
    if r != 0 {
        return Err(format!("SDL_UpdateTexture failed: {}", err()));
    }
    let _ = h;
    Ok(())
}

pub fn present(renderer: &Renderer, texture: &Texture, window: &Window) {
    let mut ww: c_int = 0;
    let mut wh: c_int = 0;
    unsafe {
        SDL_GetWindowSize(window.as_ptr(), &mut ww, &mut wh);
        SDL_RenderClear(renderer.as_ptr());
        SDL_RenderCopy(
            renderer.as_ptr(),
            texture.as_ptr(),
            core::ptr::null(),
            core::ptr::null(),
        );
        SDL_RenderPresent(renderer.as_ptr());
    }
}

/// Poll one event. Returns `Some((type, keysym))` for key events.
pub fn poll_event(buf: &mut [u8; SDL_EVENT_SIZE]) -> Option<(u32, i32)> {
    let r = unsafe { SDL_PollEvent(buf.as_mut_ptr() as *mut c_void) };
    if r == 0 {
        return None;
    }
    let ty = u32::from_ne_bytes([buf[0], buf[1], buf[2], buf[3]]);
    if ty == SDL_KEYDOWN {
        // SDL_Keysym::sym is at offset 20 on 64-bit.
        let sym = i32::from_ne_bytes([buf[20], buf[21], buf[22], buf[23]]);
        Some((ty, sym))
    } else {
        Some((ty, 0))
    }
}

pub fn delay(ms: u32) {
    unsafe { SDL_Delay(ms) }
}

pub fn ticks() -> u32 {
    unsafe { SDL_GetTicks() }
}

pub fn set_title(window: &Window, title: &str) {
    let mut c = title.as_bytes().to_vec();
    c.push(0);
    unsafe { SDL_SetWindowTitle(window.as_ptr(), c.as_ptr() as *const c_char) };
}

pub fn destroy(window: &Window, renderer: &Renderer, texture: &Texture) {
    unsafe {
        SDL_DestroyTexture(texture.as_ptr());
        SDL_DestroyRenderer(renderer.as_ptr());
        SDL_DestroyWindow(window.as_ptr());
    }
}

impl Drop for Window {
    fn drop(&mut self) {
        // Window destroyed explicitly; avoid double free.
    }
}
