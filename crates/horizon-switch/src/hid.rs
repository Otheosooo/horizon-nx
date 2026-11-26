//! libnx's HID: player 1 plus the handheld controls (libnx merges them), and the touch screen.
//! The `PadState` layout and the pad calls are the ones bevy-horizon's `input.rs` runs on hardware;
//! the touch layout follows switch/services/hid.h (checked by the size assertions below).
use crate::{
    Frame, KeyboardFrame, KeyboardTracker, MouseEvent, MouseTracker, Pointer, Raw,
    RawKeyboardSample, RawMouseSample, Touch, TouchTracker, Tracker, style,
};

#[repr(C)]
#[derive(Default, Copy, Clone)]
struct Stick {
    x: i32,
    y: i32,
}

#[repr(C)]
#[derive(Default, Copy, Clone)]
struct PadState {
    id_mask: u8,
    active_id_mask: u8,
    read_handheld: bool,
    active_handheld: bool,
    style_set: u32,
    attributes: u32,
    _pad: u32,
    buttons_cur: u64,
    buttons_old: u64,
    sticks: [Stick; 2],
    gc_triggers: [u32; 2],
}

#[repr(C)]
#[derive(Default, Copy, Clone)]
struct TouchState {
    delta_time: u64,
    attributes: u32,
    finger_id: u32,
    x: u32,
    y: u32,
    diameter_x: u32,
    diameter_y: u32,
    rotation_angle: u32,
    _reserved: u32,
}

#[repr(C)]
#[derive(Default, Copy, Clone)]
struct TouchScreenState {
    sampling_number: u64,
    count: i32,
    _reserved: u32,
    touches: [TouchState; 16],
}

#[repr(C)]
#[derive(Default, Copy, Clone)]
struct HidMouseState {
    sampling_number: u64,
    x: i32,
    y: i32,
    delta_x: i32,
    delta_y: i32,
    wheel_delta_x: i32,
    wheel_delta_y: i32,
    buttons: u32,
    attributes: u32,
}

#[repr(C)]
#[derive(Default, Copy, Clone)]
struct HidKeyboardState {
    sampling_number: u64,
    modifiers: u64,
    keys: [u64; 4],
}

const _: () = {
    assert!(size_of::<PadState>() == 56);
    assert!(size_of::<TouchState>() == 40);
    assert!(size_of::<TouchScreenState>() == 656);
    assert!(size_of::<HidMouseState>() == 40);
    assert!(size_of::<HidKeyboardState>() == 48);
};

unsafe extern "C" {
    fn padConfigureInput(max_players: u32, style_set: u32);
    fn padInitializeWithMask(state: *mut PadState, pad_mask: u64);
    fn padUpdate(state: *mut PadState);
    fn hidInitializeTouchScreen();
    fn hidGetTouchScreenStates(states: *mut TouchScreenState, count: usize) -> usize;
    fn hidInitializeMouse();
    fn hidGetMouseStates(states: *mut HidMouseState, count: usize) -> usize;
    fn hidInitializeKeyboard();
    fn hidGetKeyboardStates(states: *mut HidKeyboardState, count: usize) -> usize;
    fn appletMainLoop() -> bool;
    fn appletGetOperationMode() -> u32;
    fn appletGetFocusState() -> u32;
}

/// The applet side of libnx: HOME, sleep, docking.
pub struct Libnx;

impl Libnx {
    /// libnx's `appletMainLoop`: handles applet messages; false once the system asks the program
    /// to exit (HOME menu "close", or hbmenu returning).
    pub fn main_loop() -> bool {
        // SAFETY: plain libnx call, valid after startup.
        unsafe { appletMainLoop() }
    }

    /// Docked (1920x1080) rather than handheld (1280x720).
    pub fn docked() -> bool {
        // AppletOperationMode_Handheld = 0, _Console = 1.
        // SAFETY: plain libnx call.
        unsafe { appletGetOperationMode() == 1 }
    }

    /// The program is in the foreground (AppletFocusState_InFocus = 1).
    pub fn in_focus() -> bool {
        // SAFETY: plain libnx call.
        unsafe { appletGetFocusState() == 1 }
    }

    /// The HOME menu is open or the console is going to sleep (AppletFocusState_Background = 3).
    /// A library applet over the program (the software keyboard) is out of focus (2), not this.
    pub fn in_background() -> bool {
        // SAFETY: plain libnx call.
        unsafe { appletGetFocusState() == 3 }
    }
}

/// The touch screen alone, for the window system; `Hid::touch` is the same thing for the game.
pub struct TouchScreen {
    touch: TouchTracker,
}

impl TouchScreen {
    pub fn new() -> Self {
        // SAFETY: libnx call, safe to repeat.
        unsafe { hidInitializeTouchScreen() };
        Self {
            touch: TouchTracker::default(),
        }
    }

    pub fn poll(&mut self) -> Pointer {
        read_touch(&mut self.touch)
    }
}

impl Default for TouchScreen {
    fn default() -> Self {
        Self::new()
    }
}

/// USB HID mouse (OTG or dock), polled by the window system.
pub struct Mouse {
    tracker: MouseTracker,
}

impl Mouse {
    pub fn new() -> Self {
        // SAFETY: libnx call, safe to repeat.
        unsafe { hidInitializeMouse() };
        Self {
            tracker: MouseTracker::default(),
        }
    }

    pub fn is_connected(&self) -> bool {
        self.tracker.is_connected()
    }

    pub fn poll(&mut self) -> Vec<MouseEvent> {
        let mut states = [HidMouseState::default(); 16];
        // SAFETY: libnx writes at most `states.len()` entries into `states`.
        let n = unsafe { hidGetMouseStates(states.as_mut_ptr(), states.len()) }.min(states.len());
        let mut raw = [RawMouseSample::default(); 16];
        for (dst, src) in raw[..n].iter_mut().zip(&states[..n]) {
            *dst = RawMouseSample {
                sampling_number: src.sampling_number,
                delta_x: src.delta_x,
                delta_y: src.delta_y,
                wheel_delta_x: src.wheel_delta_x,
                wheel_delta_y: src.wheel_delta_y,
                buttons: src.buttons,
                attributes: src.attributes,
            };
        }
        self.tracker.update(&raw[..n])
    }
}

impl Default for Mouse {
    fn default() -> Self {
        Self::new()
    }
}

/// USB HID keyboard (OTG or dock), polled by the window system.
pub struct Keyboard {
    tracker: KeyboardTracker,
}

impl Keyboard {
    pub fn new() -> Self {
        // SAFETY: libnx call, safe to repeat.
        unsafe { hidInitializeKeyboard() };
        Self {
            tracker: KeyboardTracker::default(),
        }
    }

    pub fn poll(&mut self) -> KeyboardFrame {
        let mut states = [HidKeyboardState::default(); 16];
        // SAFETY: libnx writes at most `states.len()` entries into `states`.
        let n =
            unsafe { hidGetKeyboardStates(states.as_mut_ptr(), states.len()) }.min(states.len());
        let mut raw = [RawKeyboardSample::default(); 16];
        for (dst, src) in raw[..n].iter_mut().zip(&states[..n]) {
            *dst = RawKeyboardSample {
                sampling_number: src.sampling_number,
                modifiers: src.modifiers,
                keys: src.keys,
            };
        }
        self.tracker.update(&raw[..n])
    }
}

impl Default for Keyboard {
    fn default() -> Self {
        Self::new()
    }
}

/// Reads the controllers and the touch screen once per frame, on the thread that owns the game.
pub struct Hid {
    pad: PadState,
    pads: Tracker,
    touch: TouchTracker,
}

impl Hid {
    pub fn new() -> Self {
        // SAFETY: libnx's documented start-up order for the pad API; called once, from the main
        // thread, before the first padUpdate.
        let mut pad = PadState::default();
        unsafe {
            // Four players, as the game's controller reader sets (HID takes the last call): one
            // would turn splitscreen's other controllers away.
            padConfigureInput(4, style::STANDARD);
            // Player 1 (bit 0) and the handheld controls (bit 32).
            padInitializeWithMask(&mut pad, 1 | (1 << 32));
            hidInitializeTouchScreen();
        }
        Self {
            pad,
            pads: Tracker::default(),
            touch: TouchTracker::default(),
        }
    }

    /// The pad this frame. `deadzone` is the stick's rest-drift cut (0.1 suits the Switch's ~0.08).
    pub fn pad(&mut self, deadzone: f32) -> Frame {
        // SAFETY: `self.pad` is a valid, initialised PadState.
        unsafe { padUpdate(&mut self.pad) };
        let p = &self.pad;
        let raw = Raw {
            buttons: p.buttons_cur,
            sticks: [
                [p.sticks[0].x, p.sticks[0].y],
                [p.sticks[1].x, p.sticks[1].y],
            ],
            style_set: p.style_set,
        };
        self.pads.frame(raw, deadzone)
    }

    /// The controller styles after the last `pad` call, if they changed on it (to log).
    pub fn style_changed(&self) -> Option<u32> {
        self.pads.changed()
    }

    pub fn touch(&mut self) -> Pointer {
        read_touch(&mut self.touch)
    }
}

fn read_touch(tracker: &mut TouchTracker) -> Pointer {
    // SAFETY: an all-zero TouchScreenState is valid, and libnx fills at most `count` entries.
    let mut s: TouchScreenState = unsafe { core::mem::zeroed() };
    let n = unsafe { hidGetTouchScreenStates(&mut s, 1) };
    let touches: Vec<Touch> = if n == 0 {
        Vec::new()
    } else {
        s.touches[..(s.count.clamp(0, 16)) as usize]
            .iter()
            .map(|t| Touch {
                id: t.finger_id,
                x: t.x as f32,
                y: t.y as f32,
            })
            .collect()
    };
    tracker.update(&touches)
}
