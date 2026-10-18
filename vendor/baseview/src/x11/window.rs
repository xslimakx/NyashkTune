use std::cell::Cell;
use std::error::Error;
use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::Arc;
use std::thread;

use raw_window_handle::{
    HasRawDisplayHandle, HasRawWindowHandle, RawDisplayHandle, RawWindowHandle, XlibDisplayHandle,
    XlibWindowHandle,
};

use x11rb::connection::{Connection, RequestConnection};
use x11rb::protocol::xproto::{
    AtomEnum, ChangeWindowAttributesAux, ConfigureWindowAux, ConnectionExt as _, CreateGCAux,
    CreateWindowAux, EventMask, PropMode, Visualid, Window as XWindow, WindowClass,
};
use x11rb::wrapper::ConnectionExt as _;

use super::XcbConnection;
use crate::{
    Event, MouseCursor, Size, WindowEvent, WindowHandler, WindowInfo, WindowOpenOptions,
    WindowScalePolicy,
};

#[cfg(feature = "opengl")]
use crate::gl::{platform, GlContext};
use crate::x11::event_loop::EventLoop;
use crate::x11::visual_info::WindowVisualConfig;

pub struct WindowHandle {
    raw_window_handle: Option<RawWindowHandle>,
    close_requested: Arc<AtomicBool>,
    is_open: Arc<AtomicBool>,
}

impl WindowHandle {
    pub fn close(&mut self) {
        if self.raw_window_handle.take().is_some() {
            // FIXME: This will need to be changed from just setting an atomic to somehow
            // synchronizing with the window being closed (using a synchronous channel, or
            // by joining on the event loop thread).

            self.close_requested.store(true, Ordering::Relaxed);
        }
    }

    pub fn is_open(&self) -> bool {
        self.is_open.load(Ordering::Relaxed)
    }
}

unsafe impl HasRawWindowHandle for WindowHandle {
    fn raw_window_handle(&self) -> RawWindowHandle {
        if let Some(raw_window_handle) = self.raw_window_handle {
            if self.is_open.load(Ordering::Relaxed) {
                return raw_window_handle;
            }
        }

        RawWindowHandle::Xlib(XlibWindowHandle::empty())
    }
}

pub(crate) struct ParentHandle {
    close_requested: Arc<AtomicBool>,
    is_open: Arc<AtomicBool>,
}

impl ParentHandle {
    pub fn new() -> (Self, WindowHandle) {
        let close_requested = Arc::new(AtomicBool::new(false));
        let is_open = Arc::new(AtomicBool::new(true));

        let handle = WindowHandle {
            raw_window_handle: None,
            close_requested: Arc::clone(&close_requested),
            is_open: Arc::clone(&is_open),
        };

        (Self { close_requested, is_open }, handle)
    }

    pub fn parent_did_drop(&self) -> bool {
        self.close_requested.load(Ordering::Relaxed)
    }
}

impl Drop for ParentHandle {
    fn drop(&mut self) {
        self.is_open.store(false, Ordering::Relaxed);
    }
}

pub(crate) struct WindowInner {
    pub(crate) xcb_connection: XcbConnection,
    pub(super) window_id: XWindow,
    pub(crate) window_info: WindowInfo,
    visual_id: Visualid,
    mouse_cursor: Cell<MouseCursor>,

    pub(crate) close_requested: Cell<bool>,

    #[cfg(feature = "opengl")]
    gl_context: Option<GlContext>,

    // NyashkTune: software presentation (PutImage)
    gc_id: u32,
    depth: u8,
}

pub struct Window<'a> {
    pub(crate) inner: &'a WindowInner,
}

// Hack to allow sending a RawWindowHandle between threads. Do not make public
struct SendableRwh(RawWindowHandle);

unsafe impl Send for SendableRwh {}

type WindowOpenResult = Result<SendableRwh, ()>;

impl<'a> Window<'a> {
    pub fn open_parented<P, H, B>(parent: &P, options: WindowOpenOptions, build: B) -> WindowHandle
    where
        P: HasRawWindowHandle,
        H: WindowHandler + 'static,
        B: FnOnce(&mut crate::Window) -> H,
        B: Send + 'static,
    {
        // Convert parent into something that X understands
        let parent_id = match parent.raw_window_handle() {
            RawWindowHandle::Xlib(h) => h.window as u32,
            RawWindowHandle::Xcb(h) => h.window,
            h => panic!("unsupported parent handle type {:?}", h),
        };

        let (tx, rx) = mpsc::sync_channel::<WindowOpenResult>(1);

        let (parent_handle, mut window_handle) = ParentHandle::new();

        crate::nylog(&format!("open_parented parent={parent_id}"));
        crate::wait_for_windows(std::time::Duration::from_millis(1500));
        let live_guard = crate::LiveWindowGuard::new();
        thread::spawn(move || {
            let _live_guard = live_guard;
            let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                Self::window_thread(Some(parent_id), options, build, tx.clone(), Some(parent_handle))
            }));
            match r {
                Ok(Ok(())) => crate::nylog("window thread finished normally"),
                Ok(Err(e)) => crate::nylog(&format!("window thread ERROR: {e}")),
                Err(_) => crate::nylog("window thread PANICKED"),
            }
        });

        let raw_window_handle = match rx.recv() {
            Ok(Ok(h)) => h,
            other => {
                crate::nylog(&format!("window creation failed: {:?}", other.is_ok()));
                panic!("NyashkTune: could not create the GUI window (see nyashktune.log)");
            }
        };
        window_handle.raw_window_handle = Some(raw_window_handle.0);

        window_handle
    }

    pub fn open_blocking<H, B>(options: WindowOpenOptions, build: B)
    where
        H: WindowHandler + 'static,
        B: FnOnce(&mut crate::Window) -> H,
        B: Send + 'static,
    {
        let (tx, rx) = mpsc::sync_channel::<WindowOpenResult>(1);

        let thread = thread::spawn(move || {
            Self::window_thread(None, options, build, tx, None).unwrap();
        });

        let _ = rx.recv().unwrap().unwrap();

        thread.join().unwrap_or_else(|err| {
            eprintln!("Window thread panicked: {:#?}", err);
        });
    }

    fn window_thread<H, B>(
        parent: Option<u32>, options: WindowOpenOptions, build: B,
        tx: mpsc::SyncSender<WindowOpenResult>, parent_handle: Option<ParentHandle>,
    ) -> Result<(), Box<dyn Error>>
    where
        H: WindowHandler + 'static,
        B: FnOnce(&mut crate::Window) -> H,
        B: Send + 'static,
    {
        // Connect to the X server
        // FIXME: baseview error type instead of unwrap()
        crate::nylog("window_thread start");
        let xcb_connection = XcbConnection::new()?;

        // Get screen information
        let screen = xcb_connection.screen();
        let parent_id = parent.unwrap_or(screen.root);

        let gc_id = xcb_connection.conn.generate_id()?;
        xcb_connection.conn.create_gc(
            gc_id,
            parent_id,
            &CreateGCAux::new().foreground(screen.black_pixel).graphics_exposures(0),
        )?;

        let scaling = match options.scale {
            WindowScalePolicy::SystemScaleFactor => xcb_connection.get_scaling().unwrap_or(1.0),
            WindowScalePolicy::ScaleFactor(scale) => scale,
        };

        let window_info = WindowInfo::from_logical_size(options.size, scaling);

        #[cfg(feature = "opengl")]
        let visual_info =
            WindowVisualConfig::find_best_visual_config_for_gl(&xcb_connection, options.gl_config)?;

        #[cfg(not(feature = "opengl"))]
        let visual_info = WindowVisualConfig::find_best_visual_config(&xcb_connection)?;

        let window_id = xcb_connection.conn.generate_id()?;
        xcb_connection.conn.create_window(
            visual_info.visual_depth,
            window_id,
            parent_id,
            0,                                         // x coordinate of the new window
            0,                                         // y coordinate of the new window
            window_info.physical_size().width as u16,  // window width
            window_info.physical_size().height as u16, // window height
            0,                                         // window border
            WindowClass::INPUT_OUTPUT,
            visual_info.visual_id,
            &CreateWindowAux::new()
                .event_mask(
                    EventMask::EXPOSURE
                        | EventMask::POINTER_MOTION
                        | EventMask::BUTTON_PRESS
                        | EventMask::BUTTON_RELEASE
                        | EventMask::KEY_PRESS
                        | EventMask::KEY_RELEASE
                        | EventMask::STRUCTURE_NOTIFY
                        | EventMask::ENTER_WINDOW
                        | EventMask::LEAVE_WINDOW,
                )
                // As mentioned above, these two values are needed to be able to create a window
                // with a depth of 32-bits when the parent window has a different depth
                .colormap(visual_info.color_map)
                .border_pixel(0),
        )?;
        xcb_connection.conn.map_window(window_id)?;

        // Change window title
        let title = options.title;
        xcb_connection.conn.change_property8(
            PropMode::REPLACE,
            window_id,
            AtomEnum::WM_NAME,
            AtomEnum::STRING,
            title.as_bytes(),
        )?;

        xcb_connection.conn.change_property32(
            PropMode::REPLACE,
            window_id,
            xcb_connection.atoms.WM_PROTOCOLS,
            AtomEnum::ATOM,
            &[xcb_connection.atoms.WM_DELETE_WINDOW],
        )?;

        xcb_connection.conn.flush()?;

        // TODO: These APIs could use a couple tweaks now that everything is internal and there is
        //       no error handling anymore at this point. Everything is more or less unchanged
        //       compared to when raw-gl-context was a separate crate.
        #[cfg(feature = "opengl")]
        let gl_context = visual_info.fb_config.map(|fb_config| {
            use std::ffi::c_ulong;

            let window = window_id as c_ulong;
            let display = xcb_connection.dpy;

            // Because of the visual negotation we had to take some extra steps to create this context
            // NyashkTune patch: context creation can fail transiently when the host is busy
            // (stray X errors from a previous window). Retry instead of panicking, which would
            // leave the host with an empty/black plugin window.
            let mut created = None;
            for attempt in 0..10 {
                match unsafe { platform::GlContext::create(window, display, fb_config.clone()) } {
                    Ok(c) => {
                        created = Some(c);
                        break;
                    }
                    Err(e) => {
                        eprintln!("NyashkTune: OpenGL context creation failed (attempt {attempt}): {e:?}");
                        std::thread::sleep(std::time::Duration::from_millis(50));
                    }
                }
            }
            let context = created.expect("Could not create OpenGL context");
            GlContext::new(context)
        });

        // graphics context + real depth of the window for PutImage
        let sw_gc = xcb_connection.conn.generate_id()?;
        xcb_connection.conn.create_gc(sw_gc, window_id, &CreateGCAux::new())?;
        let sw_depth = xcb_connection.conn.get_geometry(window_id)?.reply()?.depth;
        let mut inner = WindowInner {
            gc_id: sw_gc,
            depth: sw_depth,
            xcb_connection,
            window_id,
            window_info,
            visual_id: visual_info.visual_id,
            mouse_cursor: Cell::new(MouseCursor::default()),

            close_requested: Cell::new(false),

            #[cfg(feature = "opengl")]
            gl_context,
        };

        let mut window = crate::Window::new(Window { inner: &mut inner });

        crate::nylog("gl context ok, building handler");
        let mut handler = build(&mut window);
        crate::nylog("handler built, entering event loop");

        // Send an initial window resized event so the user is alerted of
        // the correct dpi scaling.
        handler.on_event(&mut window, Event::Window(WindowEvent::Resized(window_info)));

        let _ = tx.send(Ok(SendableRwh(window.raw_window_handle())));

        EventLoop::new(inner, handler, parent_handle).run()?;

        Ok(())
    }

    pub fn set_mouse_cursor(&self, mouse_cursor: MouseCursor) {
        if self.inner.mouse_cursor.get() == mouse_cursor {
            return;
        }

        let xid = self.inner.xcb_connection.get_cursor(mouse_cursor).unwrap();

        if xid != 0 {
            let _ = self.inner.xcb_connection.conn.change_window_attributes(
                self.inner.window_id,
                &ChangeWindowAttributesAux::new().cursor(xid),
            );
            let _ = self.inner.xcb_connection.conn.flush();
        }

        self.inner.mouse_cursor.set(mouse_cursor);
    }

    pub fn close(&mut self) {
        self.inner.close_requested.set(true);
    }

    pub fn has_focus(&mut self) -> bool {
        unimplemented!()
    }

    pub fn focus(&mut self) {
        unimplemented!()
    }

    pub fn resize(&mut self, size: Size) {
        let scaling = self.inner.window_info.scale();
        let new_window_info = WindowInfo::from_logical_size(size, scaling);

        let _ = self.inner.xcb_connection.conn.configure_window(
            self.inner.window_id,
            &ConfigureWindowAux::new()
                .width(new_window_info.physical_size().width)
                .height(new_window_info.physical_size().height),
        );
        let _ = self.inner.xcb_connection.conn.flush();

        // This will trigger a `ConfigureNotify` event which will in turn change `self.window_info`
        // and notify the window handler about it
    }

    #[cfg(feature = "opengl")]
    pub fn gl_context(&self) -> Option<&crate::gl::GlContext> {
        self.inner.gl_context.as_ref()
    }

    /// NyashkTune: present a BGRA (little-endian 0xAARRGGBB) image at the top-left of the window.
    /// Uploads only the sub-rectangle `(x, y, w, h)` of a full BGRA frame `data` whose stride is `full_w` pixels.
    pub fn put_image_bgra_rect(&self, full_w: u32, x: u32, y: u32, w: u32, h: u32, data: &[u8]) {
        use x11rb::protocol::xproto::ImageFormat;
        let conn = &self.inner.xcb_connection.conn;
        if w == 0 || h == 0 || x + w > full_w {
            return;
        }
        let stride = full_w as usize * 4;
        if data.len() < stride * (y + h) as usize {
            return;
        }
        let row_bytes = w as usize * 4;
        let max_req = conn.maximum_request_bytes().min(1 << 20).saturating_sub(64);
        let rows_per = (max_req / row_bytes).max(1) as u32;
        let mut buf: Vec<u8> = Vec::with_capacity(row_bytes * rows_per.min(h) as usize);
        let mut yy = 0u32;
        while yy < h {
            let rows = rows_per.min(h - yy);
            buf.clear();
            for r in 0..rows {
                let o = (y + yy + r) as usize * stride + x as usize * 4;
                buf.extend_from_slice(&data[o..o + row_bytes]);
            }
            let _ = conn.put_image(
                ImageFormat::Z_PIXMAP,
                self.inner.window_id,
                self.inner.gc_id,
                w as u16,
                rows as u16,
                x as i16,
                (y + yy) as i16,
                0,
                self.inner.depth,
                &buf,
            );
            yy += rows;
        }
        let _ = conn.flush();
    }

    pub fn put_image_bgra(&self, width: u32, height: u32, data: &[u8]) {
        use x11rb::protocol::xproto::ImageFormat;
        let conn = &self.inner.xcb_connection.conn;
        let row_bytes = width as usize * 4;
        if row_bytes == 0 || data.len() < row_bytes * height as usize {
            return;
        }
        let max_req = conn.maximum_request_bytes().min(1 << 20).saturating_sub(64);
        let rows_per = (max_req / row_bytes).max(1) as u32;
        let mut y = 0u32;
        while y < height {
            let rows = rows_per.min(height - y);
            let start = y as usize * row_bytes;
            let end = start + rows as usize * row_bytes;
            let _ = conn.put_image(
                ImageFormat::Z_PIXMAP,
                self.inner.window_id,
                self.inner.gc_id,
                width as u16,
                rows as u16,
                0,
                y as i16,
                0,
                self.inner.depth,
                &data[start..end],
            );
            y += rows;
        }
        let _ = conn.flush();
    }
}

unsafe impl<'a> HasRawWindowHandle for Window<'a> {
    fn raw_window_handle(&self) -> RawWindowHandle {
        let mut handle = XlibWindowHandle::empty();

        handle.window = self.inner.window_id.into();
        handle.visual_id = self.inner.visual_id.into();

        RawWindowHandle::Xlib(handle)
    }
}

unsafe impl<'a> HasRawDisplayHandle for Window<'a> {
    fn raw_display_handle(&self) -> RawDisplayHandle {
        let display = self.inner.xcb_connection.dpy;
        let mut handle = XlibDisplayHandle::empty();

        handle.display = display as *mut c_void;
        handle.screen = unsafe { x11::xlib::XDefaultScreen(display) };

        RawDisplayHandle::Xlib(handle)
    }
}

pub fn copy_to_clipboard(_data: &str) {
    todo!()
}
