use super::{Axes, HEIGHT_PER_WIDTH, initial_size, resize_step};
use tauri::LogicalSize;

const HORIZONTAL: Axes = Axes {
    width: true,
    height: false,
};
const VERTICAL: Axes = Axes {
    width: false,
    height: true,
};
fn size(width: f64, height: f64) -> LogicalSize<f64> {
    LogicalSize::new(width, height)
}

#[test]
fn starts_with_phone_proportions_and_fits_short_screens() {
    let launched = initial_size(size(393.0, 780.0), 1200.0);
    assert!((launched.width - 393.0).abs() < 0.001);
    assert!((launched.height - 851.5).abs() < 0.001);
    for height in [716.0, 768.0, 900.0] {
        let fitted = initial_size(size(393.0, 851.5), height);
        assert!(fitted.width >= 320.0 && fitted.height < height);
    }
    assert_eq!(
        initial_size(size(1280.0, 800.0), 1200.0),
        size(1280.0, 800.0)
    );
}

#[test]
fn width_growth_leaves_height_unchanged() {
    assert_eq!(
        resize_step(size(360.0, 780.0), size(430.0, 820.0), HORIZONTAL),
        size(430.0, 780.0)
    );
}

#[test]
fn height_growth_leaves_width_unchanged() {
    assert_eq!(
        resize_step(size(360.0, 780.0), size(400.0, 860.0), VERTICAL),
        size(360.0, 860.0)
    );
}

#[test]
fn shrinking_restores_phone_proportions_after_independent_growth() {
    let smaller = resize_step(size(393.0, 851.5), size(360.0, 851.5), HORIZONTAL);
    assert!((smaller.width - 360.0).abs() < 0.001);
    assert!((smaller.height - 780.0).abs() < 0.001);
    // Previous expansion must not redefine the phone's aspect.
    let taller = resize_step(size(400.0, 1000.0), size(360.0, 1000.0), HORIZONTAL);
    assert_eq!(taller, size(360.0, 780.0));
    let wider = resize_step(size(430.0, 780.0), size(430.0, 760.0), VERTICAL);
    assert!((wider.width - 760.0 / HEIGHT_PER_WIDTH).abs() < 0.001);
    assert!((wider.height - 760.0).abs() < 0.001);
}

#[test]
fn shrinking_height_scales_width_and_respects_the_minimum() {
    let smaller = resize_step(size(393.0, 851.5), size(393.0, 780.0), VERTICAL);
    assert!((smaller.width - 360.0).abs() < 0.001);
    let minimum = resize_step(size(360.0, 780.0), size(360.0, 600.0), VERTICAL);
    assert!((minimum.width - 320.0).abs() < 0.001);
    assert!((minimum.height - 320.0 * HEIGHT_PER_WIDTH).abs() < 0.001);
}

#[test]
fn reversing_a_shrink_only_grows_the_dragged_axis() {
    let shrunk = resize_step(size(400.0, 800.0), size(360.0, 800.0), HORIZONTAL);
    assert_eq!(shrunk, size(360.0, 780.0));
    assert_eq!(
        resize_step(shrunk, size(380.0, 800.0), HORIZONTAL),
        size(380.0, 780.0)
    );
}

#[test]
fn shrinking_after_width_growth_never_temporarily_enlarges_the_height() {
    let mut current = size(430.0, 780.0);
    for width in (320..430).rev() {
        let next = resize_step(current, size(f64::from(width), current.height), HORIZONTAL);
        assert!(next.width <= current.width && next.height <= current.height);
        assert!((next.width - f64::from(width)).abs() < 0.001);
        current = next;
    }
    assert_eq!(current, size(320.0, 320.0 * HEIGHT_PER_WIDTH));
}

#[test]
fn shrinking_a_tall_or_short_window_does_not_expand_the_untouched_axis() {
    assert_eq!(
        resize_step(size(360.0, 1000.0), size(360.0, 990.0), VERTICAL),
        size(360.0, 990.0),
    );
    assert_eq!(
        resize_step(size(430.0, 600.0), size(420.0, 600.0), HORIZONTAL),
        size(420.0, 600.0),
    );
}

#[test]
fn desktop_retains_independent_axes_and_invalid_dimensions_are_ignored() {
    assert_eq!(
        resize_step(size(1280.0, 800.0), size(1000.0, 800.0), HORIZONTAL),
        size(1000.0, 800.0)
    );
    assert_eq!(
        resize_step(size(393.0, 851.5), size(f64::NAN, 851.5), HORIZONTAL),
        size(393.0, 851.5)
    );
}
