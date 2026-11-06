//! Editor-only magnetic alignment. No guide data is added to exported documents.
use numbering_core::NumberField;

/// Snap each axis independently to the nearest other field's page-space anchor.
/// Tolerances are supplied in mm, converted by the UI from a fixed screen distance.
/// Anchor guides take precedence over optional half-millimetre rounding.
pub fn snap_position(
    fields: &[NumberField],
    moving_id: u32,
    position: (f64, f64),
    tolerance: Option<(f64, f64)>,
    half_mm: bool,
) -> (f64, f64) {
    let fallback = |value: f64| {
        if half_mm {
            (value * 2.0).round() / 2.0
        } else {
            value
        }
    };
    let nearest = |value: f64, threshold: f64, axis: fn(&NumberField) -> f64| {
        fields
            .iter()
            .filter(|f| f.id != moving_id)
            .filter(|f| (axis(f) - value).abs() <= threshold)
            .min_by(|a, b| {
                (axis(a) - value)
                    .abs()
                    .total_cmp(&(axis(b) - value).abs())
                    .then_with(|| a.id.cmp(&b.id))
            })
            .map(axis)
            .unwrap_or_else(|| fallback(value))
    };
    match tolerance {
        Some((tx, ty)) => (
            nearest(position.0, tx, |f| f.x_mm),
            nearest(position.1, ty, |f| f.y_mm),
        ),
        None => (fallback(position.0), fallback(position.1)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use numbering_core::Project;

    #[test]
    fn independently_snaps_to_different_fields_and_ignores_self() {
        let mut fields = Project::default().fields;
        fields[0].x_mm = 20.1;
        fields[0].y_mm = 30.2;
        let mut second = fields[0].clone();
        second.id = 2;
        second.x_mm = 70.0;
        second.y_mm = 80.3;
        fields.push(second);
        let mut moving = fields[0].clone();
        moving.id = 3;
        moving.x_mm = 20.8;
        moving.y_mm = 80.9;
        fields.push(moving);
        assert_eq!(
            snap_position(&fields, 3, (20.8, 80.9), Some((1.0, 1.0)), true),
            (20.1, 80.3)
        );
        assert_eq!(
            snap_position(&fields, 3, (20.8, 80.9), None, false),
            (20.8, 80.9)
        );
        assert_eq!(
            snap_position(&fields, 3, (20.8, 80.9), None, true),
            (21.0, 81.0)
        );
    }

    #[test]
    fn chooses_nearest_axis_and_falls_back_outside_tolerance() {
        let mut fields = Project::default().fields;
        fields[0].x_mm = 10.0;
        let mut second = fields[0].clone();
        second.id = 2;
        second.x_mm = 11.0;
        fields.push(second);
        assert_eq!(
            snap_position(&fields, 99, (10.8, 50.3), Some((1.0, 1.0)), false),
            (11.0, 50.3)
        );
        assert_eq!(
            snap_position(&fields, 99, (10.8, 50.3), Some((0.1, 0.1)), false),
            (10.8, 50.3)
        );
        assert_eq!(
            snap_position(&fields[..1], 1, (10.4, 12.4), Some((10.0, 10.0)), false),
            (10.4, 12.4)
        );
    }
}
