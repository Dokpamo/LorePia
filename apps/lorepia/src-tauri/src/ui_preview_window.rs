//! Direction-sensitive live resizing for the isolated macOS UI preview.
//! `AppKit` receives the desired frame before painting; no resize feedback loop.

use tauri::{LogicalSize, Window};

mod native;
#[cfg(test)]
mod tests;

const HEIGHT_PER_WIDTH: f64 = 19.5 / 9.0;
const PHONE_MAX_WIDTH: f64 = 480.0;
const MIN_WIDTH: f64 = 320.0;
const MIN_HEIGHT: f64 = 552.0;
const SCREEN_RESERVE: f64 = 48.0;

#[derive(Clone, Copy)]
struct Axes {
    width: bool,
    height: bool,
}

fn resize_step(
    current: LogicalSize<f64>,
    proposed: LogicalSize<f64>,
    axes: Axes,
) -> LogicalSize<f64> {
    if [
        current.width,
        current.height,
        proposed.width,
        proposed.height,
    ]
    .iter()
    .any(|value| !value.is_finite() || *value <= 0.0)
    {
        return current;
    }
    let desired = LogicalSize::new(
        if axes.width {
            proposed.width.max(MIN_WIDTH)
        } else {
            current.width
        },
        if axes.height {
            proposed.height.max(MIN_HEIGHT)
        } else {
            current.height
        },
    );
    if current.width > PHONE_MAX_WIDTH && desired.width > PHONE_MAX_WIDTH {
        return desired;
    }
    let shrinking_width = axes.width && desired.width < current.width;
    let shrinking_height = axes.height && desired.height < current.height;
    if !shrinking_width && !shrinking_height {
        // Outward dragging changes only the grabbed axis, including reversal
        // within a drag that previously reduced both dimensions.
        return desired;
    }
    // The inward edge must never enlarge the other axis. After independent
    // growth, hold that axis until the phone aspect fits, then shrink together.
    let width = match (shrinking_width, shrinking_height) {
        (true, true) => desired.width.min(desired.height / HEIGHT_PER_WIDTH),
        (false, true) => desired.height / HEIGHT_PER_WIDTH,
        _ => desired.width,
    }
    .clamp(MIN_WIDTH, PHONE_MAX_WIDTH);
    LogicalSize::new(
        width.min(current.width),
        (width * HEIGHT_PER_WIDTH).min(current.height),
    )
}

fn initial_size(size: LogicalSize<f64>, available_height: f64) -> LogicalSize<f64> {
    if size.width > PHONE_MAX_WIDTH {
        return size;
    }
    let reserve =
        SCREEN_RESERVE.min(((available_height - MIN_WIDTH * HEIGHT_PER_WIDTH) / 2.0).max(0.0));
    let maximum = ((available_height - reserve) / HEIGHT_PER_WIDTH)
        .floor()
        .clamp(MIN_WIDTH, PHONE_MAX_WIDTH);
    let width = size.width.clamp(MIN_WIDTH, maximum);
    LogicalSize::new(width, width * HEIGHT_PER_WIDTH)
}

pub(super) fn update(window: &Window) {
    let owned = window.clone();
    let _ = window.run_on_main_thread(move || {
        if !native::register(&owned) {
            return;
        }
        let (Ok(scale), Ok(physical)) = (owned.scale_factor(), owned.outer_size()) else {
            return;
        };
        let size = physical.to_logical::<f64>(scale);
        let available = owned
            .current_monitor()
            .ok()
            .flatten()
            .map_or(size.height + SCREEN_RESERVE, |monitor| {
                f64::from(monitor.work_area().size.height) / scale
            });
        let inset = native::frame_inset(&owned);
        let content = LogicalSize::new(size.width, size.height - inset);
        let fitted = initial_size(content, available - inset);
        let fitted = LogicalSize::new(fitted.width, fitted.height + inset);
        if (fitted.width - size.width).abs() > 0.5 || (fitted.height - size.height).abs() > 0.5 {
            // Only launch is normalized. Visible-titlebar Tao uses the full
            // frame dimensions here; subsequent drags stay entirely in AppKit.
            let _ = owned.set_size(fitted);
        }
    });
}

pub(super) fn forget(label: &str) {
    native::forget(label);
}
