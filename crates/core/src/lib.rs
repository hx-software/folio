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
pub const MAX_FONT_BYTES: usize = 10 * 1024 * 1024;
pub const MIN_ARTWORK_SCALE: f64 = 10.0;
pub const MAX_ARTWORK_SCALE: f64 = 500.0;
pub const MAX_ARTWORK_OFFSET_MM: f64 = 2000.0;
pub const MAX_SPACING_MM: f64 = 50.0;
/// Largest first number: every integer up to here is exact in the browser's numbers.
pub const MAX_START: u64 = 9_007_199_254_740_991;
pub const PT_PER_MM: f64 = 72.0 / 25.4;

#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct Error(pub String);

pub type Result<T> = std::result::Result<T, Error>;

/// Binary blobs are base64 in JSON (about a third of the size of a number array) and
/// raw bytes in binary protocols. Older projects stored JSON number arrays; both load.
mod bytes_format {
    use base64::{Engine, engine::general_purpose::STANDARD};
    use serde::de::{self, SeqAccess, Visitor};
    use serde::{Deserializer, Serializer};

    pub fn serialize<S: Serializer>(bytes: &[u8], serializer: S) -> Result<S::Ok, S::Error> {
        if serializer.is_human_readable() {
            serializer.serialize_str(&STANDARD.encode(bytes))
        } else {
            serializer.serialize_bytes(bytes)
        }
    }

    struct BytesVisitor;

    impl<'de> Visitor<'de> for BytesVisitor {
        type Value = Vec<u8>;

        fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("base64 text or bytes")
        }
        fn visit_str<E: de::Error>(self, value: &str) -> Result<Vec<u8>, E> {
            STANDARD.decode(value).map_err(E::custom)
        }
        fn visit_bytes<E: de::Error>(self, value: &[u8]) -> Result<Vec<u8>, E> {
            Ok(value.to_vec())
        }
        fn visit_byte_buf<E: de::Error>(self, value: Vec<u8>) -> Result<Vec<u8>, E> {
            Ok(value)
        }
        fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Vec<u8>, A::Error> {
            let mut bytes = Vec::with_capacity(seq.size_hint().unwrap_or(0).min(1 << 20));
            while let Some(byte) = seq.next_element::<u8>()? {
                bytes.push(byte);
            }
            Ok(bytes)
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<u8>, D::Error> {
        if deserializer.is_human_readable() {
            deserializer.deserialize_any(BytesVisitor)
        } else {
            deserializer.deserialize_byte_buf(BytesVisitor)
        }
    }
}

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
    #[serde(default)]
    pub font: Option<CustomFont>,
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
    /// Extra space between the prefix and the number, in mm.
    #[serde(default)]
    pub prefix_spacing_mm: f64,
    /// Extra space between the number and the suffix, in mm.
    #[serde(default)]
    pub suffix_spacing_mm: f64,
    pub order: Order,
    /// Counts down: the same numbers are printed in the opposite order.
    #[serde(default)]
    pub reverse: bool,
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
    /// This field's own first number. `None` follows the project's "start at".
    #[serde(default)]
    pub start: Option<u64>,
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
    #[serde(with = "bytes_format")]
    pub bytes: Vec<u8>,
    pub page_index: usize,
    /// Size relative to the fitted sheet, in percent, scaled about the sheet's centre.
    #[serde(default = "default_artwork_scale")]
    pub scale_percent: f64,
    /// Shift from the centred position, in mm (right and down are positive).
    #[serde(default)]
    pub offset_x_mm: f64,
    #[serde(default)]
    pub offset_y_mm: f64,
}

impl Artwork {
    pub fn is_centered(&self) -> bool {
        self.offset_x_mm == 0.0 && self.offset_y_mm == 0.0
    }
}

fn default_artwork_scale() -> f64 {
    100.0
}

/// A user-supplied TrueType/OpenType font, embedded in the project and exported PDFs.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CustomFont {
    pub name: String,
    /// CSS family used for browser preview. Always recomputed from the bytes on load.
    pub family: String,
    #[serde(with = "bytes_format")]
    pub bytes: Vec<u8>,
}

impl CustomFont {
    pub fn new(name: impl Into<String>, bytes: Vec<u8>) -> Self {
        Self {
            name: name.into(),
            family: Self::family_for(&bytes),
            bytes,
        }
    }

    /// Content-addressed so two different fonts can never share a preview family.
    pub fn family_for(bytes: &[u8]) -> String {
        let mut hash = 0xcbf2_9ce4_8422_2325_u64;
        for byte in bytes {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(0x0100_0000_01b3);
        }
        format!("folio-font-{hash:016x}")
    }
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
    /// The complete text, for convenience and tests.
    pub text: String,
    pub prefix: String,
    pub digits: String,
    pub suffix: String,
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
                prefix_spacing_mm: 0.0,
                suffix_spacing_mm: 0.0,
                order: Order::Sequential,
                reverse: false,
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
                start: None,
            }],
            font: None,
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
        let span = (self.sequence.quantity - 1)
            .checked_mul(self.sequence.increment)
            .ok_or_else(|| Error("Sequence exceeds the integer limit.".into()))?;
        check(
            self.sequence
                .start
                .checked_add(span)
                .is_some_and(|last| last <= MAX_START),
            "Sequence exceeds the integer limit.",
        )?;
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
        check(
            [
                self.sequence.prefix_spacing_mm,
                self.sequence.suffix_spacing_mm,
            ]
            .iter()
            .all(|v| v.is_finite() && (0.0..=MAX_SPACING_MM).contains(v)),
            "Prefix and suffix spacing must be between 0 and 50 mm.",
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
            check(
                field.start.is_none_or(|start| {
                    start
                        .checked_add(span)
                        .is_some_and(|last| last <= MAX_START)
                }),
                "A field's start number is too large for this quantity and increment.",
            )?;
        }
        if let Some(artwork) = &self.artwork {
            check(
                artwork.scale_percent.is_finite()
                    && (MIN_ARTWORK_SCALE..=MAX_ARTWORK_SCALE).contains(&artwork.scale_percent),
                "Artwork scale must be between 10% and 500%.",
            )?;
            check(
                [artwork.offset_x_mm, artwork.offset_y_mm]
                    .iter()
                    .all(|v| v.is_finite() && v.abs() <= MAX_ARTWORK_OFFSET_MM),
                "Artwork position must be within 2000 mm of the centre.",
            )?;
            check(
                !artwork.bytes.is_empty() && artwork.bytes.len() <= MAX_ARTWORK_BYTES,
                "Artwork must be non-empty and no larger than 20 MB.",
            )?;
            check(artwork.name.len() <= 255, "Artwork filename is too long.")?;
        }
        if let Some(font) = &self.font {
            check(
                !font.bytes.is_empty() && font.bytes.len() <= MAX_FONT_BYTES,
                "Font must be non-empty and no larger than 10 MB.",
            )?;
            check(font.name.len() <= 255, "Font filename is too long.")?;
            check(
                font.family.len() <= 40
                    && font
                        .family
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || c == '-'),
                "Invalid font identifier.",
            )?;
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
        self.field_number(self.sequence.start, self.item_index(sheet, slot)?)
    }

    /// Which item (0-based, in numbering order) prints at a sheet position.
    pub fn item_index(&self, sheet: usize, slot: usize) -> Option<u64> {
        if sheet >= self.sheet_count() || slot >= self.slots() {
            return None;
        }
        let index = match self.sequence.order {
            Order::Sequential => sheet.checked_mul(self.slots())?.checked_add(slot)?,
            Order::CutAndStack => slot.checked_mul(self.sheet_count())?.checked_add(sheet)?,
        } as u64;
        (index < self.sequence.quantity).then_some(index)
    }

    /// The number printed for an item by a sequence starting at `start`. With
    /// `reverse`, the same run of numbers is printed from the highest to the lowest.
    pub fn field_number(&self, start: u64, index: u64) -> Option<u64> {
        let index = if self.sequence.reverse {
            self.sequence.quantity.checked_sub(1)?.checked_sub(index)?
        } else {
            index
        };
        start.checked_add(index.checked_mul(self.sequence.increment)?)
    }

    /// A field's own first number, or the project's when it has none.
    pub fn field_start(&self, field: &NumberField) -> u64 {
        field.start.unwrap_or(self.sequence.start)
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

    /// The prefix, zero-padded number, and suffix, in drawing order.
    pub fn format_parts(&self, number: u64) -> (String, String, String) {
        (
            self.sequence.prefix.clone(),
            format!("{:0width$}", number, width = self.sequence.padding.min(20)),
            self.sequence.suffix.clone(),
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
            let Some(index) = self.item_index(sheet, slot) else {
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
                let number = self
                    .field_number(self.field_start(field), index)
                    .ok_or_else(|| Error("A number exceeds the supported range.".into()))?;
                let (prefix, digits, suffix) = self.format_parts(number);
                result.push(PlacedNumber {
                    field: field.clone(),
                    text: self.format_number(number),
                    prefix,
                    digits,
                    suffix,
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
        let mut project: Self =
            serde_json::from_slice(bytes).map_err(|e| Error(format!("Invalid project: {e}")))?;
        if let Some(font) = &mut project.font {
            font.family = CustomFont::family_for(&font.bytes);
        }
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
    fn prefix_and_suffix_parts_and_spacing_are_validated() {
        let mut p = Project::default();
        p.sequence.suffix = " pcs".into();
        p.sequence.prefix_spacing_mm = 2.5;
        p.sequence.suffix_spacing_mm = 4.0;
        let placed = &p.layout_sheet(0).unwrap()[0];
        assert_eq!(
            (&placed.prefix[..], &placed.digits[..], &placed.suffix[..]),
            ("No. ", "0001", " pcs")
        );
        assert_eq!(placed.text, "No. 0001 pcs");
        assert_eq!(Project::from_bytes(&p.to_bytes().unwrap()).unwrap(), p);
        for bad in [-0.1, 50.1, f64::NAN, f64::INFINITY] {
            let mut q = p.clone();
            q.sequence.prefix_spacing_mm = bad;
            assert!(q.validate().is_err(), "{bad}");
            q = p.clone();
            q.sequence.suffix_spacing_mm = bad;
            assert!(q.validate().is_err(), "{bad}");
        }
        let mut legacy = serde_json::to_value(Project::default()).unwrap();
        let sequence = legacy["sequence"].as_object_mut().unwrap();
        sequence.remove("prefix_spacing_mm");
        sequence.remove("suffix_spacing_mm");
        let loaded = Project::from_bytes(&serde_json::to_vec(&legacy).unwrap()).unwrap();
        assert_eq!(loaded.sequence.prefix_spacing_mm, 0.0);
        assert_eq!(loaded.sequence.suffix_spacing_mm, 0.0);
    }

    #[test]
    fn blobs_use_base64_and_legacy_number_arrays_still_load() {
        let mut p = Project::default();
        p.artwork = Some(Artwork {
            name: "a.png".into(),
            kind: ArtworkKind::Png,
            bytes: vec![0, 1, 2, 250, 255],
            page_index: 0,
            scale_percent: 100.0,
            offset_x_mm: 0.0,
            offset_y_mm: 0.0,
        });
        p.font = Some(CustomFont::new("font.ttf", vec![9, 8, 7, 6]));
        let json = serde_json::to_value(&p).unwrap();
        assert_eq!(json["artwork"]["bytes"], "AAEC+v8=");
        assert!(json["font"]["bytes"].is_string());
        assert_eq!(Project::from_bytes(&p.to_bytes().unwrap()).unwrap(), p);
        let mut legacy = json;
        legacy["artwork"]["bytes"] = serde_json::json!([0, 1, 2, 250, 255]);
        let loaded = Project::from_bytes(&serde_json::to_vec(&legacy).unwrap()).unwrap();
        assert_eq!(loaded.artwork.unwrap().bytes, vec![0, 1, 2, 250, 255]);
        let mut broken = serde_json::to_value(&p).unwrap();
        broken["artwork"]["bytes"] = "not base64!".into();
        assert!(Project::from_bytes(&serde_json::to_vec(&broken).unwrap()).is_err());
    }

    #[test]
    fn custom_font_identity_is_recomputed_and_validated() {
        let mut p = Project::default();
        let font = CustomFont::new("a.ttf", vec![1, 2, 3]);
        assert_ne!(font.family, CustomFont::new("b.ttf", vec![1, 2, 4]).family);
        assert_eq!(font.family, CustomFont::new("c.ttf", vec![1, 2, 3]).family);
        p.font = Some(font.clone());
        p.validate().unwrap();
        let mut json = serde_json::to_value(&p).unwrap();
        json["font"]["family"] = "x'); } body { display:none".into();
        let loaded = Project::from_bytes(&serde_json::to_vec(&json).unwrap()).unwrap();
        assert_eq!(loaded.font.unwrap().family, font.family);
        p.font.as_mut().unwrap().family = "bad family".into();
        assert!(p.validate().is_err());
        p.font = Some(CustomFont::new("big.ttf", vec![0; MAX_FONT_BYTES + 1]));
        assert!(p.validate().is_err());
        p.font = Some(CustomFont::new("empty.ttf", vec![]));
        assert!(p.validate().is_err());
    }

    #[test]
    fn fields_can_start_at_their_own_numbers() {
        let mut p = Project::default();
        p.sequence.start = 100;
        p.sequence.increment = 5;
        p.fields[0].start = Some(7000);
        let mut stub = p.fields[0].clone();
        stub.id = 2;
        stub.start = None;
        let mut third = stub.clone();
        third.id = 3;
        third.start = Some(0);
        p.fields.extend([stub, third]);
        let texts = |sheet: usize| -> Vec<String> {
            p.layout_sheet(sheet)
                .unwrap()
                .into_iter()
                .map(|n| n.text)
                .collect()
        };
        assert_eq!(
            texts(0)[..6],
            [
                "No. 7000", "No. 0100", "No. 0000", "No. 7005", "No. 0105", "No. 0005"
            ]
        );
        let last_sheet = p.sheet_count() - 1;
        assert_eq!(texts(last_sheet).len(), 3 * (100 % 6));
        assert_eq!(p.field_start(&p.fields[0]), 7000);
        assert_eq!(p.field_start(&p.fields[1]), 100);
        // Every field covers its own range exactly once, in either numbering order.
        for order in [Order::Sequential, Order::CutAndStack] {
            p.sequence.order = order;
            let mut seen: Vec<Vec<u64>> = vec![Vec::new(); 3];
            for sheet in 0..p.sheet_count() {
                for (i, placed) in p.layout_sheet(sheet).unwrap().iter().enumerate() {
                    seen[i % 3].push(placed.digits.parse().unwrap());
                }
            }
            for (field, mut numbers) in seen.into_iter().enumerate() {
                numbers.sort_unstable();
                let start = p.field_start(&p.fields[field]);
                assert_eq!(numbers, (0..100).map(|i| start + i * 5).collect::<Vec<_>>());
            }
        }
    }

    #[test]
    fn field_starts_are_validated_and_legacy_projects_load() {
        let mut p = Project::default();
        p.fields[0].start = Some(MAX_START);
        assert!(p.validate().is_err(), "quantity pushes it past the limit");
        p.sequence.quantity = 1;
        p.validate().unwrap();
        p.fields[0].start = Some(MAX_START + 1);
        assert!(p.validate().is_err());
        p.fields[0].start = Some(u64::MAX);
        assert!(p.validate().is_err());
        p.fields[0].start = Some(5);
        p.sequence.start = MAX_START + 1;
        assert!(p.validate().is_err());
        p.sequence.start = 1;
        assert_eq!(Project::from_bytes(&p.to_bytes().unwrap()).unwrap(), p);
        let mut legacy = serde_json::to_value(Project::default()).unwrap();
        legacy["fields"][0].as_object_mut().unwrap().remove("start");
        let loaded = Project::from_bytes(&serde_json::to_vec(&legacy).unwrap()).unwrap();
        assert_eq!(loaded.fields[0].start, None);
    }

    #[test]
    fn reverse_prints_the_same_numbers_in_opposite_order() {
        let mut p = Project::default();
        p.sequence.quantity = 10;
        p.sequence.start = 100;
        p.sequence.increment = 5;
        p.grid.columns = 2;
        p.grid.rows = 2;
        p.fields[0].start = Some(7000);
        let mut stub = p.fields[0].clone();
        stub.id = 2;
        stub.start = None;
        p.fields.push(stub);
        let digits = |p: &Project| -> Vec<(u64, u64)> {
            (0..p.sheet_count())
                .flat_map(|sheet| p.layout_sheet(sheet).unwrap())
                .map(|n| (n.field.id as u64, n.digits.parse().unwrap()))
                .collect()
        };
        for order in [Order::Sequential, Order::CutAndStack] {
            p.sequence.order = order;
            p.sequence.reverse = false;
            let forward = digits(&p);
            p.sequence.reverse = true;
            let backward = digits(&p);
            assert_eq!(forward.len(), backward.len());
            for (field, start) in [(1, 7000), (2, 100)] {
                let pick = |all: &[(u64, u64)]| -> Vec<u64> {
                    all.iter()
                        .filter(|(f, _)| *f == field)
                        .map(|(_, n)| *n)
                        .collect()
                };
                let (a, b) = (pick(&forward), pick(&backward));
                assert_eq!(a.len(), 10);
                assert_eq!(a.first(), Some(&start));
                assert_eq!(
                    b.first(),
                    Some(&(start + 45)),
                    "first item gets the highest number"
                );
                if order == Order::Sequential {
                    assert_eq!(b.last(), Some(&start), "last item gets the lowest");
                }
                let mirrored: Vec<u64> = a.iter().map(|n| 2 * start + 45 - n).collect();
                assert_eq!(b, mirrored);
            }
        }
        assert_eq!(p.number_at(0, 0), Some(145));
        p.sequence.quantity = 1;
        assert_eq!(p.number_at(0, 0), Some(100), "a single item is unaffected");
        let mut legacy = serde_json::to_value(Project::default()).unwrap();
        legacy["sequence"]
            .as_object_mut()
            .unwrap()
            .remove("reverse");
        let loaded = Project::from_bytes(&serde_json::to_vec(&legacy).unwrap()).unwrap();
        assert!(!loaded.sequence.reverse);
        assert_eq!(Project::from_bytes(&p.to_bytes().unwrap()).unwrap(), p);
    }

    #[test]
    fn artwork_scale_is_validated_and_legacy_projects_load() {
        let mut p = Project::default();
        p.artwork = Some(Artwork {
            name: "a.png".into(),
            kind: ArtworkKind::Png,
            bytes: vec![1],
            page_index: 0,
            scale_percent: 100.0,
            offset_x_mm: 0.0,
            offset_y_mm: 0.0,
        });
        p.validate().unwrap();
        for ok in [MIN_ARTWORK_SCALE, 250.0, MAX_ARTWORK_SCALE] {
            p.artwork.as_mut().unwrap().scale_percent = ok;
            p.validate().unwrap();
        }
        for bad in [0.0, 9.9, 500.1, f64::NAN, f64::INFINITY] {
            p.artwork.as_mut().unwrap().scale_percent = bad;
            assert!(p.validate().is_err(), "{bad}");
        }
        p.artwork.as_mut().unwrap().scale_percent = 150.0;
        assert_eq!(Project::from_bytes(&p.to_bytes().unwrap()).unwrap(), p);
        let mut legacy = serde_json::to_value(&p).unwrap();
        legacy["artwork"]
            .as_object_mut()
            .unwrap()
            .remove("scale_percent");
        let loaded = Project::from_bytes(&serde_json::to_vec(&legacy).unwrap()).unwrap();
        assert_eq!(loaded.artwork.unwrap().scale_percent, 100.0);
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
