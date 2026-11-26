use std::cell::Cell;
use std::collections::VecDeque;
use std::marker::PhantomData;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

use horizon_switch::{Keyboard, Libnx, Mouse, MouseButtonId, MouseEvent, Pointer, TouchScreen};

use super::keyboard;
use super::{DeviceId, MonitorHandle, PlatformSpecificEventLoopAttributes, WindowId};
use crate::dpi::{PhysicalPosition, PhysicalSize};
use crate::error::EventLoopError;
use crate::event::{
    self, ElementState, MouseButton, MouseScrollDelta, StartCause, TouchPhase, WindowEvent,
};
use crate::event_loop::{self, ControlFlow, DeviceEvents};
use crate::window::{
    CustomCursor as RootCustomCursor, CustomCursorSource, Theme, WindowId as RootWindowId,
};

#[cfg(target_os = "horizon")]
unsafe extern "C" {
    fn horizon_nx_menu_input();
    fn horizon_nx_set_mouse_cursor_active(active: bool);
}

/// The longest the loop sleeps between looks at HID, the applet and the proxy: the Switch has no
/// event to wait on, and a thread must never spin (a busy one starves lower-priority threads).
const POLL: Duration = Duration::from_millis(4);

pub struct EventLoop<T> {
    window_target: event_loop::ActiveEventLoop,
    user_events_sender: mpsc::Sender<T>,
    user_events_receiver: mpsc::Receiver<T>,
}

impl<T: 'static> EventLoop<T> {
    pub(crate) fn new(_: &PlatformSpecificEventLoopAttributes) -> Result<Self, EventLoopError> {
        let (user_events_sender, user_events_receiver) = mpsc::channel();
        Ok(Self {
            window_target: event_loop::ActiveEventLoop {
                p: ActiveEventLoop {
                    control_flow: Cell::new(ControlFlow::default()),
                    exit: Cell::new(false),
                    redraw: Arc::new(AtomicBool::new(false)),
                    window_created: Cell::new(false),
                    cursor_grabbed: Arc::new(AtomicBool::new(false)),
                    cursor_override: Arc::new(Mutex::new(None)),
                },
                _marker: PhantomData,
            },
            user_events_sender,
            user_events_receiver,
        })
    }

    pub fn run<F>(self, mut handler: F) -> Result<(), EventLoopError>
    where
        F: FnMut(event::Event<T>, &event_loop::ActiveEventLoop),
    {
        let target = &self.window_target;
        let id = RootWindowId(WindowId);
        let mut touch = TouchScreen::new();
        let mut mouse = Mouse::new();
        let mut keyboard = Keyboard::new();
        let mut cause = StartCause::Init;
        let mut size = super::screen_size();
        let mut focused = false;
        let mut cursor: Option<PhysicalPosition<f64>> = None;
        let mut mouse_cursor =
            PhysicalPosition::new(size.width as f64 / 2.0, size.height as f64 / 2.0);
        let mut button_down = false;
        let mut closing = false;

        loop {
            handler(event::Event::NewEvents(cause), target);
            if cause == StartCause::Init {
                handler(event::Event::Resumed, target);
            }

            // The window was created in a callback: tell the app its size, as other backends do.
            if target.p.window_created.replace(false) {
                size = super::screen_size();
                mouse_cursor = clamp_pos(mouse_cursor, size);
                handler(
                    event::Event::WindowEvent { window_id: id, event: WindowEvent::Resized(size) },
                    target,
                );
            }

            // Docking or undocking changes the screen's size.
            let now_size = super::screen_size();
            if now_size != size {
                size = now_size;
                mouse_cursor = clamp_pos(mouse_cursor, size);
                handler(
                    event::Event::WindowEvent { window_id: id, event: WindowEvent::Resized(size) },
                    target,
                );
            }

            if let Some(pos) = target.p.cursor_override.lock().unwrap().take() {
                let clamped = clamp_pos(pos, size);
                mouse_cursor = clamped;
                cursor = Some(clamped);
            }

            // The applet's messages are the game's to handle (`horizon_nx::process_applet_messages`
            // in its `about_to_wait`: a request to exit starts its closing save), so the loop only
            // reads the state they leave. A library applet over the game (the software keyboard)
            // is out of focus, not in the background: the game keeps its focus through it.
            let in_focus = !Libnx::in_background();
            if in_focus != focused {
                focused = in_focus;
                handler(
                    event::Event::WindowEvent { window_id: id, event: WindowEvent::Focused(focused) },
                    target,
                );
            }

            let touch_events =
                pointer_events(touch.poll(), size, &mut cursor, &mut button_down);
            if !touch_events.is_empty() {
                #[cfg(target_os = "horizon")]
                // SAFETY: plain atomic store in horizon-nx.
                unsafe {
                    horizon_nx_set_mouse_cursor_active(false);
                }
                if let Some(at) = cursor {
                    mouse_cursor = clamp_pos(at, size);
                }
                for pointer_event in touch_events {
                    handler(
                        event::Event::WindowEvent { window_id: id, event: pointer_event },
                        target,
                    );
                }
            }

            let mouse_events = mouse.poll();
            if !mouse_events.is_empty() {
                #[cfg(target_os = "horizon")]
                // SAFETY: plain functions in horizon-nx.
                unsafe {
                    horizon_nx_menu_input();
                    horizon_nx_set_mouse_cursor_active(true);
                }
                let grabbed = target.p.cursor_grabbed.load(Ordering::Relaxed);
                let device_id = event::DeviceId(DeviceId);
                for mouse_event in mouse_events {
                    match mouse_event {
                        MouseEvent::Motion { dx, dy } => {
                            if grabbed {
                                handler(
                                    event::Event::DeviceEvent {
                                        device_id,
                                        event: event::DeviceEvent::MouseMotion {
                                            delta: (dx as f64, dy as f64),
                                        },
                                    },
                                    target,
                                );
                            } else {
                                mouse_cursor = clamp_pos(
                                    PhysicalPosition::new(
                                        mouse_cursor.x + dx as f64,
                                        mouse_cursor.y + dy as f64,
                                    ),
                                    size,
                                );
                                cursor = Some(mouse_cursor);
                                handler(
                                    event::Event::WindowEvent {
                                        window_id: id,
                                        event: WindowEvent::CursorMoved {
                                            device_id,
                                            position: mouse_cursor,
                                        },
                                    },
                                    target,
                                );
                            }
                        },
                        MouseEvent::Button { button, pressed } => {
                            let button = match button {
                                MouseButtonId::Left => MouseButton::Left,
                                MouseButtonId::Right => MouseButton::Right,
                                MouseButtonId::Middle => MouseButton::Middle,
                                MouseButtonId::Forward => MouseButton::Forward,
                                MouseButtonId::Back => MouseButton::Back,
                            };
                            handler(
                                event::Event::WindowEvent {
                                    window_id: id,
                                    event: WindowEvent::MouseInput {
                                        device_id,
                                        state: if pressed {
                                            ElementState::Pressed
                                        } else {
                                            ElementState::Released
                                        },
                                        button,
                                    },
                                },
                                target,
                            );
                        },
                        MouseEvent::Wheel { dx, dy } => {
                            handler(
                                event::Event::WindowEvent {
                                    window_id: id,
                                    event: WindowEvent::MouseWheel {
                                        device_id,
                                        delta: MouseScrollDelta::LineDelta(dx as f32, dy as f32),
                                        phase: TouchPhase::Moved,
                                    },
                                },
                                target,
                            );
                        },
                    }
                }
            }

            let kbd_frame = keyboard.poll();
            if kbd_frame.modifiers_changed || !kbd_frame.events.is_empty() {
                #[cfg(target_os = "horizon")]
                // SAFETY: plain function in horizon-nx.
                unsafe {
                    horizon_nx_menu_input();
                }
                let device_id = event::DeviceId(DeviceId);
                if kbd_frame.modifiers_changed {
                    handler(
                        event::Event::WindowEvent {
                            window_id: id,
                            event: WindowEvent::ModifiersChanged(keyboard::modifiers_from_frame(
                                kbd_frame.modifiers,
                                kbd_frame.pressed_modifier_keys,
                            )),
                        },
                        target,
                    );
                }
                for change in kbd_frame.events {
                    if let Some(key_event) = keyboard::to_key_event(change) {
                        handler(
                            event::Event::WindowEvent {
                                window_id: id,
                                event: WindowEvent::KeyboardInput {
                                    device_id,
                                    event: key_event,
                                    is_synthetic: false,
                                },
                            },
                            target,
                        );
                    }
                }
            }

            while let Ok(user) = self.user_events_receiver.try_recv() {
                handler(event::Event::UserEvent(user), target);
            }

            if target.p.redraw.swap(false, Ordering::Relaxed) {
                handler(
                    event::Event::WindowEvent { window_id: id, event: WindowEvent::RedrawRequested },
                    target,
                );
            }

            handler(event::Event::AboutToWait, target);

            if target.p.exiting() {
                break;
            }

            // HOME or sleep: that was the last frame. The system suspends the program inside this
            // call (or it waits there without drawing) until it is in front again.
            #[cfg(target_os = "horizon")]
            if Libnx::in_background() {
                if focused {
                    focused = false;
                    handler(
                        event::Event::WindowEvent {
                            window_id: id,
                            event: WindowEvent::Focused(false),
                        },
                        target,
                    );
                }
                handler(event::Event::Suspended, target);
                if horizon_nx::suspend_until_out_of_background().is_none() && !closing {
                    // Closed from the HOME menu while suspended.
                    closing = true;
                    handler(
                        event::Event::WindowEvent {
                            window_id: id,
                            event: WindowEvent::CloseRequested,
                        },
                        target,
                    );
                }
                handler(event::Event::Resumed, target);
                cause = StartCause::Poll;
                continue;
            }

            let start = Instant::now();
            let requested_resume = match target.p.control_flow() {
                ControlFlow::Poll => {
                    cause = StartCause::Poll;
                    // Still give other threads the core.
                    std::thread::yield_now();
                    continue;
                },
                ControlFlow::Wait => None,
                ControlFlow::WaitUntil(instant) => Some(instant),
            };
            // HID has no wake-up: sleep one short step, then look again.
            if !target.p.redraw.load(Ordering::Relaxed) {
                let left = requested_resume.map(|t| t.saturating_duration_since(start));
                std::thread::sleep(left.map_or(POLL, |d| d.min(POLL)));
            }
            cause = match requested_resume {
                Some(requested_resume) if Instant::now() >= requested_resume => {
                    StartCause::ResumeTimeReached { start, requested_resume }
                },
                _ => StartCause::WaitCancelled { start, requested_resume },
            };
        }

        handler(event::Event::LoopExiting, target);
        Ok(())
    }

    pub fn window_target(&self) -> &event_loop::ActiveEventLoop {
        &self.window_target
    }

    pub fn create_proxy(&self) -> EventLoopProxy<T> {
        EventLoopProxy { user_events_sender: self.user_events_sender.clone() }
    }
}

fn clamp_pos(pos: PhysicalPosition<f64>, size: PhysicalSize<u32>) -> PhysicalPosition<f64> {
    PhysicalPosition::new(
        pos.x.clamp(0.0, size.width.saturating_sub(1) as f64),
        pos.y.clamp(0.0, size.height.saturating_sub(1) as f64),
    )
}

/// The touch screen as a mouse: the finger moves the cursor and holds the left button down, a
/// second finger scrolls, and tapping it is a right click.
fn pointer_events(
    pointer: Pointer,
    size: PhysicalSize<u32>,
    cursor: &mut Option<PhysicalPosition<f64>>,
    button_down: &mut bool,
) -> Vec<WindowEvent> {
    let device_id = event::DeviceId(DeviceId);
    let mut out = Vec::new();
    if let Some([x, y]) = pointer.at {
        let at = PhysicalPosition::new(x as f64 * size.width as f64, y as f64 * size.height as f64);
        if *cursor != Some(at) {
            *cursor = Some(at);
            out.push(WindowEvent::CursorMoved { device_id, position: at });
        }
    }
    if pointer.down != *button_down {
        *button_down = pointer.down;
        out.push(WindowEvent::MouseInput {
            device_id,
            state: if pointer.down { ElementState::Pressed } else { ElementState::Released },
            button: MouseButton::Left,
        });
    }
    if pointer.right_tap {
        for state in [ElementState::Pressed, ElementState::Released] {
            out.push(WindowEvent::MouseInput { device_id, state, button: MouseButton::Right });
        }
    }
    if pointer.scroll != 0.0 {
        out.push(WindowEvent::MouseWheel {
            device_id,
            delta: MouseScrollDelta::PixelDelta(PhysicalPosition::new(0.0, pointer.scroll as f64)),
            phase: TouchPhase::Moved,
        });
    }
    out
}

pub struct EventLoopProxy<T: 'static> {
    user_events_sender: mpsc::Sender<T>,
}

impl<T> EventLoopProxy<T> {
    pub fn send_event(&self, event: T) -> Result<(), event_loop::EventLoopClosed<T>> {
        self.user_events_sender
            .send(event)
            .map_err(|mpsc::SendError(x)| event_loop::EventLoopClosed(x))
    }
}

impl<T> Clone for EventLoopProxy<T> {
    fn clone(&self) -> Self {
        Self { user_events_sender: self.user_events_sender.clone() }
    }
}

impl<T> Unpin for EventLoopProxy<T> {}

pub struct ActiveEventLoop {
    control_flow: Cell<ControlFlow>,
    exit: Cell<bool>,
    pub(super) redraw: Arc<AtomicBool>,
    pub(super) window_created: Cell<bool>,
    pub(super) cursor_grabbed: Arc<AtomicBool>,
    pub(super) cursor_override: Arc<Mutex<Option<PhysicalPosition<f64>>>>,
}

impl ActiveEventLoop {
    pub fn create_custom_cursor(&self, source: CustomCursorSource) -> RootCustomCursor {
        let _ = source.inner;
        RootCustomCursor { inner: super::PlatformCustomCursor }
    }

    pub fn primary_monitor(&self) -> Option<MonitorHandle> {
        Some(MonitorHandle)
    }

    pub fn available_monitors(&self) -> VecDeque<MonitorHandle> {
        VecDeque::from([MonitorHandle])
    }

    #[inline]
    pub fn listen_device_events(&self, _allowed: DeviceEvents) {}

    #[inline]
    pub fn system_theme(&self) -> Option<Theme> {
        None
    }

    #[cfg(feature = "rwh_06")]
    #[inline]
    pub fn raw_display_handle_rwh_06(
        &self,
    ) -> Result<rwh_06::RawDisplayHandle, rwh_06::HandleError> {
        Ok(rwh_06::RawDisplayHandle::Horizon(rwh_06::HorizonDisplayHandle::new()))
    }

    pub fn set_control_flow(&self, control_flow: ControlFlow) {
        self.control_flow.set(control_flow)
    }

    pub fn control_flow(&self) -> ControlFlow {
        self.control_flow.get()
    }

    pub(crate) fn exit(&self) {
        self.exit.set(true);
    }

    pub(crate) fn exiting(&self) -> bool {
        self.exit.get()
    }

    pub(crate) fn owned_display_handle(&self) -> OwnedDisplayHandle {
        OwnedDisplayHandle
    }
}

#[derive(Clone)]
pub(crate) struct OwnedDisplayHandle;

impl OwnedDisplayHandle {
    #[cfg(feature = "rwh_06")]
    #[inline]
    pub fn raw_display_handle_rwh_06(
        &self,
    ) -> Result<rwh_06::RawDisplayHandle, rwh_06::HandleError> {
        Ok(rwh_06::HorizonDisplayHandle::new().into())
    }
}
