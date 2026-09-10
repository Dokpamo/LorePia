//! Optional delegate hooks; existing Tao delegate ownership/methods are retained.
use std::collections::BTreeMap;
use std::ffi::{c_char, c_void};
use std::sync::{Mutex, OnceLock};

use super::{Axes, resize_step};
use tauri::{LogicalSize, Window};

struct RegisteredWindow {
    native: usize,
    axes: Option<Axes>,
}
static WINDOWS: Mutex<BTreeMap<String, RegisteredWindow>> = Mutex::new(BTreeMap::new());
static HOOKS: OnceLock<bool> = OnceLock::new();

#[repr(C)]
#[derive(Clone, Copy)]
struct Size {
    width: f64,
    height: f64,
}
#[repr(C)]
#[derive(Clone, Copy)]
struct Rect {
    origin: Size,
    size: Size,
}

#[allow(
    unsafe_code,
    clashing_extern_declarations,
    reason = "objc_msgSend is polymorphic; each declaration matches its fixed AppKit selector ABI"
)]
#[link(name = "objc")]
unsafe extern "C" {
    #[link_name = "sel_registerName"]
    fn selector(name: *const c_char) -> *const c_void;
    #[link_name = "object_getClass"]
    fn object_class(object: *mut c_void) -> *mut c_void;
    #[link_name = "class_getInstanceMethod"]
    fn instance_method(class: *mut c_void, selector: *const c_void) -> *mut c_void;
    #[link_name = "class_addMethod"]
    fn add_method(
        class: *mut c_void,
        selector: *const c_void,
        implementation: *const c_void,
        types: *const c_char,
    ) -> bool;
    #[link_name = "objc_msgSend"]
    fn pointer(object: *mut c_void, selector: *const c_void) -> *mut c_void;
    #[link_name = "objc_msgSend"]
    fn flag(object: *mut c_void, selector: *const c_void) -> i8;
    #[link_name = "objc_msgSend"]
    fn point(object: *mut c_void, selector: *const c_void) -> Size;
    #[link_name = "objc_msgSend"]
    fn set_pointer(object: *mut c_void, selector: *const c_void, value: *mut c_void);
    #[link_name = "objc_msgSend"]
    fn set_size(object: *mut c_void, selector: *const c_void, value: Size);
    #[cfg(target_arch = "aarch64")]
    #[link_name = "objc_msgSend"]
    fn frame(object: *mut c_void, selector: *const c_void) -> Rect;
    #[cfg(target_arch = "x86_64")]
    #[link_name = "objc_msgSend_stret"]
    fn frame_stret(result: *mut Rect, object: *mut c_void, selector: *const c_void);
}

#[allow(unsafe_code)]
fn window_rect(window: *mut c_void, property: *const c_char) -> Rect {
    // SAFETY: callers pass a live NSWindow/NSScreen on AppKit's main thread
    // and a no-argument NSRect getter. Intel uses a different struct-return ABI.
    unsafe {
        #[cfg(target_arch = "aarch64")]
        {
            frame(window, selector(property))
        }
        #[cfg(target_arch = "x86_64")]
        {
            let mut result = Rect {
                origin: Size {
                    width: 0.0,
                    height: 0.0,
                },
                size: Size {
                    width: 0.0,
                    height: 0.0,
                },
            };
            frame_stret(&raw mut result, window, selector(property));
            result
        }
    }
}

pub(super) fn frame_inset(window: &Window) -> f64 {
    window.ns_window().map_or(0.0, |native| {
        if native.is_null() {
            return 0.0;
        }
        let outer = window_rect(native, c"frame".as_ptr()).size;
        let content = window_rect(native, c"contentLayoutRect".as_ptr()).size;
        (outer.height - content.height).max(0.0)
    })
}

#[allow(unsafe_code)]
extern "C" fn start(_delegate: *mut c_void, _selector: *const c_void, notification: *mut c_void) {
    // SAFETY: AppKit delivers this NSNotification synchronously on the main
    // thread; object is its live NSWindow and the point is in window coordinates.
    let (window, mouse) = unsafe {
        let window = pointer(notification, selector(c"object".as_ptr()));
        if window.is_null() {
            return;
        }
        (
            window,
            point(
                window,
                selector(c"mouseLocationOutsideOfEventStream".as_ptr()),
            ),
        )
    };
    let size = window_rect(window, c"frame".as_ptr()).size;
    let axes = Axes {
        width: mouse.width <= 14.0 || mouse.width >= size.width - 14.0,
        height: mouse.height <= 14.0 || mouse.height >= size.height - 14.0,
    };
    if let Ok(mut states) = WINDOWS.lock()
        && let Some(state) = states
            .values_mut()
            .find(|state| state.native == window as usize)
    {
        state.axes = (axes.width || axes.height).then_some(axes);
    }
}

#[allow(unsafe_code)]
extern "C" fn end(_delegate: *mut c_void, _selector: *const c_void, notification: *mut c_void) {
    // SAFETY: same notification ABI and main-thread lifetime as start.
    let window = unsafe { pointer(notification, selector(c"object".as_ptr())) };
    if let Ok(mut states) = WINDOWS.lock()
        && let Some(state) = states
            .values_mut()
            .find(|state| state.native == window as usize)
    {
        state.axes = None;
    }
}

#[allow(unsafe_code)]
extern "C" fn resize(
    _delegate: *mut c_void,
    _selector: *const c_void,
    window: *mut c_void,
    proposed: Size,
) -> Size {
    let Ok(states) = WINDOWS.lock() else {
        return proposed;
    };
    let Some(state) = states
        .values()
        .find(|state| state.native == window as usize)
    else {
        return proposed;
    };
    // SAFETY: AppKit calls this fixed NSWindowDelegate ABI with a live NSWindow.
    // Programmatic sizing, maximize and fullscreen are not live border drags.
    if unsafe { flag(window, selector(c"inLiveResize".as_ptr())) } == 0 {
        return proposed;
    }
    let current = window_rect(window, c"frame".as_ptr()).size;
    let content = window_rect(window, c"contentLayoutRect".as_ptr()).size;
    let inset = (current.height - content.height).max(0.0);
    let axes = state.axes.unwrap_or(Axes {
        width: (proposed.width - current.width).abs() > 0.5,
        height: (proposed.height - current.height).abs() > 0.5,
    });
    let result = resize_step(
        LogicalSize::new(current.width, current.height - inset),
        LogicalSize::new(proposed.width, proposed.height - inset),
        axes,
    );
    // Launch is fitted to the display separately. Reapplying that aspect fit
    // here would enlarge a short frame during an inward drag. Shrinking is
    // bounded by the current frame; AppKit retains its native screen limits.
    Size {
        width: result.width,
        height: result.height + inset,
    }
}

#[allow(unsafe_code)]
fn install(delegate: *mut c_void) -> bool {
    *HOOKS.get_or_init(|| {
        // SAFETY: registration is on the main thread. Only absent optional
        // methods are added; no Tao implementation is replaced or swizzled.
        // Each callback returns default behavior for unregistered windows.
        unsafe {
            let class = object_class(delegate);
            let hooks = [
                (
                    c"windowWillResize:toSize:",
                    resize as *const c_void,
                    c"{CGSize=dd}@:@{CGSize=dd}",
                ),
                (
                    c"windowWillStartLiveResize:",
                    start as *const c_void,
                    c"v@:@",
                ),
                (c"windowDidEndLiveResize:", end as *const c_void, c"v@:@"),
            ];
            if hooks
                .iter()
                .any(|(name, _, _)| !instance_method(class, selector(name.as_ptr())).is_null())
            {
                return false;
            }
            hooks.iter().all(|(name, implementation, types)| {
                add_method(
                    class,
                    selector(name.as_ptr()),
                    *implementation,
                    types.as_ptr(),
                )
            })
        }
    })
}

#[allow(unsafe_code)]
pub(super) fn register(window: &Window) -> bool {
    if WINDOWS
        .lock()
        .map_or(true, |states| states.contains_key(window.label()))
    {
        return false;
    }
    let Ok(native) = window.ns_window() else {
        return false;
    };
    if native.is_null() {
        return false;
    }
    // SAFETY: caller is dispatched on AppKit's main thread; Tauri retains the
    // window and delegate. No Objective-C object ownership is changed. Setting
    // the same delegate refreshes AppKit's optional-method cache once.
    unsafe {
        let delegate = pointer(native, selector(c"delegate".as_ptr()));
        if delegate.is_null() || !install(delegate) {
            return false;
        }
        if let Ok(mut states) = WINDOWS.lock() {
            states.insert(
                window.label().to_owned(),
                RegisteredWindow {
                    native: native as usize,
                    axes: None,
                },
            );
        } else {
            return false;
        }
        set_size(
            native,
            selector(c"setResizeIncrements:".as_ptr()),
            Size {
                width: 1.0,
                height: 1.0,
            },
        );
        set_pointer(native, selector(c"setDelegate:".as_ptr()), delegate);
    }
    true
}

pub(super) fn forget(label: &str) {
    if let Ok(mut states) = WINDOWS.lock() {
        states.remove(label);
    }
}
