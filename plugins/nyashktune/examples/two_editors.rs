//! Two plugin editors in one process (like a DAW with two instances): how long does opening the
//! second one take while the first is open, and how long does closing one block the host?
use nih_plug::prelude::*;
use nih_plug_egui::{create_egui_editor, egui, EguiState};
use std::sync::Arc;
use std::time::Instant;
use x11rb::connection::Connection;
use x11rb::protocol::xproto::{ConnectionExt, CreateWindowAux, WindowClass};

struct Ctx;
impl GuiContext for Ctx {
    fn plugin_api(&self) -> PluginApi {
        PluginApi::Vst3
    }
    fn request_resize(&self) -> bool {
        true
    }
    unsafe fn raw_begin_set_parameter(&self, _: ParamPtr) {}
    unsafe fn raw_set_parameter_normalized(&self, _: ParamPtr, _: f32) {}
    unsafe fn raw_end_set_parameter(&self, _: ParamPtr) {}
    fn get_state(&self) -> PluginState {
        unimplemented!()
    }
    fn set_state(&self, _: PluginState) {}
}

fn main() {
    let (conn, screen) = x11rb::connect(None).expect("X");
    let root = conn.setup().roots[screen].root;
    let mk = || {
        let w = conn.generate_id().unwrap();
        conn.create_window(0, w, root, 0, 0, 800, 500, 0, WindowClass::INPUT_OUTPUT, 0, &CreateWindowAux::new()).unwrap();
        conn.map_window(w).unwrap();
        conn.flush().unwrap();
        w
    };
    let (p1, p2) = (mk(), mk());
    let editor = || create_egui_editor(EguiState::from_size(800, 500), (), |_, _| {}, |ctx, _, _| {
        egui::CentralPanel::default().show(ctx, |ui| ui.label("hello"));
    }).unwrap();
    let (e1, e2) = (editor(), editor());
    let t = Instant::now();
    let h1 = e1.spawn(ParentWindowHandle::X11Window(p1), Arc::new(Ctx));
    println!("open first:  {:6.0} ms", t.elapsed().as_secs_f64() * 1e3);
    std::thread::sleep(std::time::Duration::from_millis(800));
    let t = Instant::now();
    let h2 = e2.spawn(ParentWindowHandle::X11Window(p2), Arc::new(Ctx));
    println!("open second: {:6.0} ms  (first still open)", t.elapsed().as_secs_f64() * 1e3);
    std::thread::sleep(std::time::Duration::from_millis(800));
    let t = Instant::now();
    drop(h1);
    println!("close first: {:6.0} ms  (second still open)", t.elapsed().as_secs_f64() * 1e3);
    std::thread::sleep(std::time::Duration::from_millis(300));
    let t = Instant::now();
    let h1 = e1.spawn(ParentWindowHandle::X11Window(p1), Arc::new(Ctx));
    println!("reopen first:{:6.0} ms", t.elapsed().as_secs_f64() * 1e3);
    std::thread::sleep(std::time::Duration::from_millis(500));
    let t = Instant::now();
    drop(h2);
    drop(h1);
    println!("close both:  {:6.0} ms", t.elapsed().as_secs_f64() * 1e3);
}
