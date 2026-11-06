//! Portable document model and deterministic, UI-independent numbering rules.
use serde::{Deserialize, Serialize};

pub const SCHEMA_VERSION: u32 = 1;
pub const MAX_ITEMS: u64 = 100_000;
pub const MAX_FIELDS: usize = 256;
pub const MAX_ARTWORK_BYTES: usize = 20 * 1024 * 1024;
pub const MAX_PROJECT_BYTES: usize = 100 * 1024 * 1024;
pub const MAX_EXPORT_SHEETS: usize = 500;
pub const MAX_NUMBERS_PER_SHEET: usize = 2_000;
pub const MAX_NUMBERS_PER_EXPORT: usize = 200_000;
pub const PT_PER_MM: f64 = 72.0 / 25.4;

#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct Error(pub String);

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Project {
    pub schema_version: u32,
    pub name: String,
    pub page: Page,
    pub sequence: Sequence,
    #[serde(default)]
    pub arrangement: Arrangement,
    pub grid: Grid,
    pub fields: Vec<NumberField>,
    pub artwork: Option<Artwork>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Page {
    pub width_mm: f64,
    pub height_mm: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Sequence {
    pub start: u64,
    pub increment: u64,
    pub quantity: u64,
    pub padding: usize,
    pub prefix: String,
    pub suffix: String,
    pub order: Order,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Order {
    #[default]
    Sequential,
    CutAndStack,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Arrangement {
    #[default]
    Grid,
    Free,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Grid {
    pub columns: u32,
    pub rows: u32,
    pub margin_mm: f64,
    pub gap_mm: f64,
}

/// A field is relative to each grid cell, or the page in free placement mode.
/// All fields in a cell (or free-placement page) share its sequence.
/// Positions refer to the text's baseline anchor, not its bounding box.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NumberField {
    pub id: u32,
    pub x_mm: f64,
    pub y_mm: f64,
    pub font_size_pt: f64,
    pub rotation: Rotation,
    pub alignment: Alignment,
    pub color: String,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Rotation {
    #[default]
    Deg0,
    Deg90,
    Deg180,
    Deg270,
}

impl Rotation {
    pub fn degrees(self) -> u16 {
        match self {
            Self::Deg0 => 0,
            Self::Deg90 => 90,
            Self::Deg180 => 180,
            Self::Deg270 => 270,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Alignment {
    #[default]
    Left,
    Center,
    Right,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Artwork {
    pub name: String,
    pub kind: ArtworkKind,
    pub bytes: Vec<u8>,
    pub page_index: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ArtworkKind {
    Pdf,
    Png,
    Jpeg,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PlacedNumber {
    pub field: NumberField,
    pub text: String,
    pub x_mm: f64,
    pub y_mm: f64,
}

impl Default for Project {
    fn default() -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            name: "Untitled numbering".into(),
            page: Page {
                width_mm: 210.0,
                height_mm: 297.0,
            },
            sequence: Sequence {
                start: 1,
                increment: 1,
                quantity: 100,
                padding: 4,
                prefix: "No. ".into(),
                suffix: String::new(),
                order: Order::Sequential,
            },
            arrangement: Arrangement::Grid,
            grid: Grid {
                columns: 2,
                rows: 3,
                margin_mm: 12.0,
                gap_mm: 4.0,
            },
            fields: vec![NumberField {
                id: 1,
                x_mm: 8.0,
                y_mm: 12.0,
                font_size_pt: 14.0,
                rotation: Rotation::Deg0,
                alignment: Alignment::Left,
                color: "#172b3a".into(),
            }],
            artwork: None,
        }
    }
}

impl Project {
    pub fn validate(&self) -> Result<()> {
        let check = |condition, message: &str| {
            if condition {
                Ok(())
            } else {
                Err(Error(message.into()))
            }
        };
        check(
            self.schema_version == SCHEMA_VERSION,
            "Unsupported project version.",
        )?;
        check(self.name.len() <= 200, "Project name is too long.")?;
        for dimension in [self.page.width_mm, self.page.height_mm] {
            check(
                dimension.is_finite() && (10.0..=2000.0).contains(&dimension),
                "Page dimensions must be between 10 and 2000 mm.",
            )?;
        }
        check(
            (1..=MAX_ITEMS).contains(&self.sequence.quantity),
            "Quantity must be 1–100,000.",
        )?;
        check(self.sequence.increment > 0, "Increment must be positive.")?;
        self.sequence
            .start
            .checked_add(
                (self.sequence.quantity - 1)
                    .checked_mul(self.sequence.increment)
                    .ok_or_else(|| Error("Sequence exceeds the integer limit.".into()))?,
            )
            .ok_or_else(|| Error("Sequence exceeds the integer limit.".into()))?;
        check(
            self.sequence.padding <= 20,
            "Padding cannot exceed 20 digits.",
        )?;
        check(
            self.sequence.prefix.len() <= 100 && self.sequence.suffix.len() <= 100,
            "Prefixes and suffixes cannot exceed 100 bytes each.",
        )?;
        check(
            !self
                .sequence
                .prefix
                .chars()
                .chain(self.sequence.suffix.chars())
                .any(char::is_control),
            "Prefixes and suffixes cannot contain control characters.",
        )?;
        if self.arrangement == Arrangement::Grid {
            check(
                (1..=20).contains(&self.grid.columns) && (1..=20).contains(&self.grid.rows),
                "Grid dimensions must be between 1 and 20.",
            )?;
            check(
                self.grid.margin_mm.is_finite()
                    && self.grid.margin_mm >= 0.0
                    && self.grid.gap_mm.is_finite()
                    && self.grid.gap_mm >= 0.0,
                "Margins and gaps must be finite and non-negative.",
            )?;
        }
        let (width, height) = self.cell_size();
        check(
            width >= 5.0 && height >= 5.0,
            "Grid cells must be at least 5 mm across.",
        )?;
        check(
            !self.fields.is_empty() && self.fields.len() <= MAX_FIELDS,
            "A project must have between 1 and 256 number fields.",
        )?;
        check(
            self.slots() * self.fields.len() <= MAX_NUMBERS_PER_SHEET,
            "Reduce the grid or field count: at most 2,000 number positions per sheet.",
        )?;
        let mut ids = std::collections::HashSet::new();
        for field in &self.fields {
            check(ids.insert(field.id), "Number field IDs must be unique.")?;
            check(
                field.x_mm.is_finite()
                    && field.y_mm.is_finite()
                    && (0.0..=width).contains(&field.x_mm)
                    && (0.0..=height).contains(&field.y_mm),
                "Number field anchors must be inside their placement area (cell or page).",
            )?;
            check(
                field.font_size_pt.is_finite() && (1.0..=300.0).contains(&field.font_size_pt),
                "Font size must be between 1 and 300 points.",
            )?;
            check(
                parse_color(&field.color).is_some(),
                "Invalid field colour; use #RRGGBB.",
            )?;
        }
        if let Some(artwork) = &self.artwork {
            check(
                !artwork.bytes.is_empty() && artwork.bytes.len() <= MAX_ARTWORK_BYTES,
                "Artwork must be non-empty and no larger than 20 MB.",
            )?;
            check(artwork.name.len() <= 255, "Artwork filename is too long.")?;
        }
        Ok(())
    }

    pub fn slots(&self) -> usize {
        match self.arrangement {
            Arrangement::Grid => self.grid.columns as usize * self.grid.rows as usize,
            Arrangement::Free => 1,
        }
    }

    pub fn sheet_count(&self) -> usize {
        (self.sequence.quantity as usize).div_ceil(self.slots().max(1))
    }

    pub fn cell_size(&self) -> (f64, f64) {
        if self.arrangement == Arrangement::Free {
            return (self.page.width_mm, self.page.height_mm);
        }
        let g = &self.grid;
        (
            (self.page.width_mm
                - 2.0 * g.margin_mm
                - g.columns.saturating_sub(1) as f64 * g.gap_mm)
                / g.columns.max(1) as f64,
            (self.page.height_mm - 2.0 * g.margin_mm - g.rows.saturating_sub(1) as f64 * g.gap_mm)
                / g.rows.max(1) as f64,
        )
    }

    /// Row-major cell order. Cut-and-stack assumes sheets stacked in ascending order,
    /// then cut and gathered in that same cell order; unused tail positions stay blank.
    pub fn number_at(&self, sheet: usize, slot: usize) -> Option<u64> {
        if sheet >= self.sheet_count() || slot >= self.slots() {
            return None;
        }
        let index = match self.sequence.order {
            Order::Sequential => sheet.checked_mul(self.slots())?.checked_add(slot)?,
            Order::CutAndStack => slot.checked_mul(self.sheet_count())?.checked_add(sheet)?,
        } as u64;
        if index >= self.sequence.quantity {
            return None;
        }
        self.sequence
            .start
            .checked_add(index.checked_mul(self.sequence.increment)?)
    }

    pub fn format_number(&self, number: u64) -> String {
        format!(
            "{}{:0width$}{}",
            self.sequence.prefix,
            number,
            self.sequence.suffix,
            width = self.sequence.padding.min(20)
        )
    }

    pub fn layout_sheet(&self, sheet: usize) -> Result<Vec<PlacedNumber>> {
        self.validate()?;
        if sheet >= self.sheet_count() {
            return Err(Error("Sheet is out of range.".into()));
        }
        let (cell_width, cell_height) = self.cell_size();
        let mut result = Vec::new();
        for slot in 0..self.slots() {
            let Some(number) = self.number_at(sheet, slot) else {
                continue;
            };
            let (origin_x, origin_y) = match self.arrangement {
                Arrangement::Grid => (
                    self.grid.margin_mm
                        + (slot % self.grid.columns as usize) as f64
                            * (cell_width + self.grid.gap_mm),
                    self.grid.margin_mm
                        + (slot / self.grid.columns as usize) as f64
                            * (cell_height + self.grid.gap_mm),
                ),
                Arrangement::Free => (0.0, 0.0),
            };
            for field in &self.fields {
                result.push(PlacedNumber {
                    field: field.clone(),
                    text: self.format_number(number),
                    x_mm: origin_x + field.x_mm,
                    y_mm: origin_y + field.y_mm,
                });
            }
        }
        Ok(result)
    }

    pub fn to_bytes(&self) -> Result<Vec<u8>> {
        self.validate()?;
        serde_json::to_vec(self).map_err(|e| Error(e.to_string()))
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        if bytes.len() > MAX_PROJECT_BYTES {
            return Err(Error("Project exceeds 100 MB.".into()));
        }
        let project: Self =
            serde_json::from_slice(bytes).map_err(|e| Error(format!("Invalid project: {e}")))?;
        project.validate()?;
        Ok(project)
    }
}

pub fn parse_color(value: &str) -> Option<[u8; 3]> {
    let hex = value.strip_prefix('#')?;
    if hex.len() != 6 || !hex.is_ascii() {
        return None;
    }
    Some([
        u8::from_str_radix(&hex[0..2], 16).ok()?,
        u8::from_str_radix(&hex[2..4], 16).ok()?,
        u8::from_str_radix(&hex[4..6], 16).ok()?,
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_valid_and_round_trips() {
        let p = Project::default();
        assert_eq!(Project::from_bytes(&p.to_bytes().unwrap()).unwrap(), p);
        assert_eq!(p.sheet_count(), 17);
        assert_eq!(p.format_number(1), "No. 0001");
    }

    #[test]
    fn old_projects_default_to_grid_and_free_projects_round_trip() {
        let mut legacy = serde_json::to_value(Project::default()).unwrap();
        legacy.as_object_mut().unwrap().remove("arrangement");
        assert_eq!(
            Project::from_bytes(&serde_json::to_vec(&legacy).unwrap())
                .unwrap()
                .arrangement,
            Arrangement::Grid
        );
        let mut p = Project::default();
        p.arrangement = Arrangement::Free;
        assert_eq!(Project::from_bytes(&p.to_bytes().unwrap()).unwrap(), p);
    }

    #[test]
    fn free_placement_uses_page_coordinates_and_one_sequence_per_sheet() {
        let mut p = Project::default();
        p.arrangement = Arrangement::Free;
        p.fields[0].x_mm = 175.3;
        p.fields[0].y_mm = 250.2;
        let mut stub = p.fields[0].clone();
        stub.id = 2;
        stub.x_mm = 20.0;
        p.fields.push(stub);
        assert_eq!(p.cell_size(), (210.0, 297.0));
        assert_eq!(p.slots(), 1);
        assert_eq!(p.sheet_count(), 100);
        let positions = p.layout_sheet(1).unwrap();
        assert_eq!(positions.len(), 2);
        assert_eq!((positions[0].x_mm, positions[0].y_mm), (175.3, 250.2));
        assert_eq!(positions[0].text, "No. 0002");
        assert_eq!(positions[0].text, positions[1].text);
        p.sequence.order = Order::CutAndStack;
        assert_eq!(p.layout_sheet(1).unwrap(), positions);
        p.fields[0].x_mm = 211.0;
        assert!(p.validate().is_err());
    }

    #[test]
    fn cut_stack_and_sequential_cover_every_item_once() {
        for order in [Order::Sequential, Order::CutAndStack] {
            for quantity in 1..150 {
                let mut p = Project::default();
                p.sequence.order = order;
                p.sequence.quantity = quantity;
                p.sequence.start = 7;
                p.sequence.increment = 3;
                let mut values = Vec::new();
                for sheet in 0..p.sheet_count() {
                    for slot in 0..p.slots() {
                        if let Some(n) = p.number_at(sheet, slot) {
                            values.push(n);
                        }
                    }
                }
                values.sort();
                assert_eq!(values, (0..quantity).map(|i| 7 + i * 3).collect::<Vec<_>>());
            }
        }
    }

    #[test]
    fn cut_stack_gathers_in_order_with_blanks_at_tail() {
        let mut p = Project::default();
        p.sequence.quantity = 8;
        p.sequence.order = Order::CutAndStack;
        assert_eq!(
            (0..6).map(|s| p.number_at(0, s)).collect::<Vec<_>>(),
            vec![Some(1), Some(3), Some(5), Some(7), None, None]
        );
        let project = &p;
        let gathered: Vec<_> = (0..6)
            .flat_map(|slot| (0..2).filter_map(move |sheet| project.number_at(sheet, slot)))
            .collect();
        assert_eq!(gathered, (1..=8).collect::<Vec<_>>());
    }

    #[test]
    fn ticket_and_stub_share_number() {
        let mut p = Project::default();
        let mut stub = p.fields[0].clone();
        stub.id = 2;
        stub.x_mm = 50.0;
        p.fields.push(stub);
        let layout = p.layout_sheet(0).unwrap();
        assert_eq!(layout[0].text, layout[1].text);
        assert_ne!(layout[0].x_mm, layout[1].x_mm);
        assert_eq!(layout[2].text, "No. 0002");
    }

    #[test]
    fn rejects_bad_documents() {
        let mut p = Project::default();
        p.sequence.start = u64::MAX;
        assert!(p.validate().is_err());
        p = Project::default();
        p.grid.columns = 0;
        assert!(p.validate().is_err());
        p = Project::default();
        p.page.width_mm = f64::NAN;
        assert!(p.validate().is_err());
        p = Project::default();
        p.fields[0].x_mm = 5000.0;
        assert!(p.validate().is_err());
        p = Project::default();
        p.schema_version = 99;
        assert!(p.validate().is_err());
        assert!(Project::from_bytes(br#"{"schema_version":1}"#).is_err());
        assert_eq!(parse_color("#ab12ff"), Some([171, 18, 255]));
        assert_eq!(parse_color("#🔥xx"), None);
    }

    proptest::proptest! {
        #![proptest_config(proptest::test_runner::Config::with_cases(96))]
        #[test]
        fn arbitrary_grids_never_lose_or_duplicate_numbers(
            columns in 1_u32..=20, rows in 1_u32..=20,
            quantity in 1_u64..10_000, start in 0_u64..1_000_000,
            increment in 1_u64..100, cut_stack in proptest::bool::ANY,
        ) {
            let mut p = Project::default();
            p.grid.columns = columns; p.grid.rows = rows;
            p.sequence.quantity = quantity; p.sequence.start = start;
            p.sequence.increment = increment;
            p.sequence.order = if cut_stack { Order::CutAndStack } else { Order::Sequential };
            let mut actual = Vec::new();
            for sheet in 0..p.sheet_count() {
                for slot in 0..p.slots() {
                    if let Some(n) = p.number_at(sheet, slot) { actual.push(n); }
                }
            }
            actual.sort_unstable();
            proptest::prop_assert_eq!(actual, (0..quantity).map(|i| start + i * increment).collect::<Vec<_>>());
            proptest::prop_assert_eq!(p.number_at(p.sheet_count(), 0), None);
            proptest::prop_assert_eq!(p.number_at(0, p.slots()), None);
        }
    }
}
