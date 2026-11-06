use numbering_core::{Arrangement, NumberField, Project};
use std::collections::BTreeSet;

#[derive(Clone)]
pub struct Editor {
    pub project: Project,
    pub selected: u32,
    pub selected_ids: BTreeSet<u32>,
    pub revision: u64,
    past: Vec<Project>,
    future: Vec<Project>,
}

impl Default for Editor {
    fn default() -> Self {
        Self {
            project: Project::default(),
            selected: 1,
            selected_ids: BTreeSet::from([1]),
            revision: 0,
            past: vec![],
            future: vec![],
        }
    }
}

impl Editor {
    // History contains layout settings only, not megabytes of duplicate artwork.
    fn snapshot(&self) -> Project {
        Project {
            schema_version: self.project.schema_version,
            name: self.project.name.clone(),
            page: self.project.page.clone(),
            sequence: self.project.sequence.clone(),
            arrangement: self.project.arrangement,
            grid: self.project.grid.clone(),
            fields: self.project.fields.clone(),
            font: None,
            // Only the view (scale and position) is kept: the bytes are too large to copy.
            artwork: self
                .project
                .artwork
                .as_ref()
                .map(|a| numbering_core::Artwork {
                    name: String::new(),
                    kind: a.kind,
                    bytes: Vec::new(),
                    page_index: a.page_index,
                    scale_percent: a.scale_percent,
                    offset_x_mm: a.offset_x_mm,
                    offset_y_mm: a.offset_y_mm,
                }),
        }
    }

    /// Swaps in a history snapshot, keeping the current artwork file and font.
    fn restore(&mut self, mut p: Project) {
        let current = self.project.artwork.take();
        p.artwork = match (current, p.artwork.take()) {
            (Some(mut art), Some(view)) => {
                art.scale_percent = view.scale_percent;
                art.offset_x_mm = view.offset_x_mm;
                art.offset_y_mm = view.offset_y_mm;
                Some(art)
            }
            (current, _) => current,
        };
        p.font = self.project.font.take();
        self.project = p;
        self.reconcile_selection();
        self.revision += 1;
    }

    pub fn checkpoint(&mut self) {
        self.past.push(self.snapshot());
        if self.past.len() > 50 {
            self.past.remove(0);
        }
        self.future.clear();
    }

    pub fn edit(&mut self, change: impl FnOnce(&mut Project)) {
        self.checkpoint();
        change(&mut self.project);
        self.revision += 1;
    }

    pub fn field(&self) -> Option<&NumberField> {
        self.project.fields.iter().find(|f| f.id == self.selected)
    }

    /// Sets the artwork's shift from centre without recording history (callers checkpoint).
    pub fn set_artwork_offset(&mut self, x: f64, y: f64) {
        let limit = numbering_core::MAX_ARTWORK_OFFSET_MM;
        if let Some(art) = &mut self.project.artwork {
            art.offset_x_mm = x.clamp(-limit, limit);
            art.offset_y_mm = y.clamp(-limit, limit);
            self.revision += 1;
        }
    }

    pub fn edit_field(&mut self, mut change: impl FnMut(&mut NumberField)) {
        let ids = self.selected_ids.clone();
        self.edit(|p| {
            for field in p.fields.iter_mut().filter(|f| ids.contains(&f.id)) {
                change(field);
            }
        });
    }

    pub fn select(&mut self, id: u32, additive: bool) {
        if !self.project.fields.iter().any(|f| f.id == id) {
            return;
        }
        if additive && self.selected_ids.contains(&id) {
            if self.selected_ids.len() > 1 {
                self.selected_ids.remove(&id);
            }
        } else {
            if !additive {
                self.selected_ids.clear();
            }
            self.selected_ids.insert(id);
            self.selected = id;
        }
        self.reconcile_selection();
    }

    pub fn select_all(&mut self) {
        self.selected_ids = self.project.fields.iter().map(|f| f.id).collect();
    }

    /// Replace the selection (marquee). Never leaves it empty: the primary field stays.
    pub fn set_selection(&mut self, ids: impl IntoIterator<Item = u32>) {
        let primary = self.selected;
        self.selected_ids = ids
            .into_iter()
            .filter(|id| self.project.fields.iter().any(|f| f.id == *id))
            .collect();
        if self.selected_ids.is_empty() {
            self.selected_ids.insert(primary);
        }
        self.reconcile_selection();
    }

    /// The font is project-wide data like the artwork: not part of undo history.
    pub fn set_font(&mut self, font: Option<numbering_core::CustomFont>) {
        self.project.font = font;
        self.revision += 1;
    }

    fn reconcile_selection(&mut self) {
        self.selected_ids
            .retain(|id| self.project.fields.iter().any(|f| f.id == *id));
        if self.selected_ids.is_empty() {
            self.selected_ids.insert(self.project.fields[0].id);
        }
        if !self.selected_ids.contains(&self.selected) {
            self.selected = *self.selected_ids.first().unwrap();
        }
    }

    pub fn selection_origins(&self) -> Vec<(u32, f64, f64)> {
        self.project
            .fields
            .iter()
            .filter(|f| self.selected_ids.contains(&f.id))
            .map(|f| (f.id, f.x_mm, f.y_mm))
            .collect()
    }

    /// Clamp the group's translation once, preserving distances at page/cell edges.
    pub fn translate_from(&mut self, origins: &[(u32, f64, f64)], dx: f64, dy: f64) {
        let (w, h) = self.project.cell_size();
        // Valid anchors always lie inside the area. Derive bounds from the selection,
        // rather than clamping each field independently and crushing its spacing.
        let left = origins
            .iter()
            .map(|(_, x, _)| *x)
            .reduce(f64::min)
            .unwrap_or(0.0);
        let right = origins
            .iter()
            .map(|(_, x, _)| *x)
            .reduce(f64::max)
            .unwrap_or(0.0);
        let top = origins
            .iter()
            .map(|(_, _, y)| *y)
            .reduce(f64::min)
            .unwrap_or(0.0);
        let bottom = origins
            .iter()
            .map(|(_, _, y)| *y)
            .reduce(f64::max)
            .unwrap_or(0.0);
        let dx = dx.clamp(-left, (w - right).max(-left));
        let dy = dy.clamp(-top, (h - bottom).max(-top));
        for (id, x, y) in origins {
            if let Some(field) = self.project.fields.iter_mut().find(|f| f.id == *id) {
                field.x_mm = x + dx;
                field.y_mm = y + dy;
            }
        }
        self.revision += 1;
    }

    pub fn move_selection(&mut self, dx: f64, dy: f64) {
        self.checkpoint();
        self.translate_from(&self.selection_origins(), dx, dy);
    }

    pub fn set_arrangement(&mut self, arrangement: Arrangement) {
        if self.project.arrangement == arrangement {
            return;
        }
        self.edit(|p| {
            // Keep the first cell's anchors in the same physical place when going free.
            let offset = if arrangement == Arrangement::Free {
                p.grid.margin_mm
            } else {
                -p.grid.margin_mm
            };
            p.arrangement = arrangement;
            let (w, h) = p.cell_size();
            for field in &mut p.fields {
                field.x_mm = (field.x_mm + offset).clamp(0.0, w.max(0.0));
                field.y_mm = (field.y_mm + offset).clamp(0.0, h.max(0.0));
            }
        });
    }

    pub fn replace(&mut self, project: Project) {
        self.selected = project.fields.first().map_or(1, |f| f.id);
        self.selected_ids = BTreeSet::from([self.selected]);
        self.project = project;
        self.revision += 1;
        self.past.clear();
        self.future.clear();
    }

    pub fn add_field(&mut self) {
        if self.project.fields.len() >= numbering_core::MAX_FIELDS {
            return;
        }
        let mut field = self
            .field()
            .cloned()
            .unwrap_or_else(|| Project::default().fields.remove(0));
        field.id = (1..=numbering_core::MAX_FIELDS as u32 + 1)
            .find(|id| !self.project.fields.iter().any(|f| f.id == *id))
            .unwrap();
        let (w, h) = self.project.cell_size();
        field.x_mm = (field.x_mm + 5.0).min(w.max(0.0));
        field.y_mm = (field.y_mm + 5.0).min(h.max(0.0));
        self.selected = field.id;
        self.selected_ids = BTreeSet::from([field.id]);
        self.edit(|p| p.fields.push(field));
    }

    pub fn delete_field(&mut self) {
        if self.project.fields.len() <= 1 {
            return;
        }
        let mut ids = self.selected_ids.clone();
        if ids.len() == self.project.fields.len() {
            ids.remove(&self.project.fields[0].id);
        }
        self.edit(|p| p.fields.retain(|f| !ids.contains(&f.id)));
        self.reconcile_selection();
    }

    pub fn undo(&mut self) {
        if let Some(p) = self.past.pop() {
            self.future.push(self.snapshot());
            self.restore(p);
        }
    }

    pub fn redo(&mut self) {
        if let Some(p) = self.future.pop() {
            self.past.push(self.snapshot());
            self.restore(p);
        }
    }

    pub fn can_undo(&self) -> bool {
        !self.past.is_empty()
    }
    pub fn can_redo(&self) -> bool {
        !self.future.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn history_restores_settings_without_duplicating_artwork() {
        let mut e = Editor::default();
        e.project.artwork = Some(numbering_core::Artwork {
            name: "test.png".into(),
            kind: numbering_core::ArtworkKind::Png,
            bytes: vec![1, 2, 3],
            page_index: 0,
            scale_percent: 100.0,
            offset_x_mm: 0.0,
            offset_y_mm: 0.0,
        });
        e.edit_field(|f| f.x_mm = 20.0);
        assert!(e.past[0].artwork.as_ref().unwrap().bytes.is_empty());
        e.undo();
        assert_eq!(e.field().unwrap().x_mm, 8.0);
        assert_eq!(e.project.artwork.as_ref().unwrap().bytes, vec![1, 2, 3]);
        e.redo();
        assert_eq!(e.field().unwrap().x_mm, 20.0);
        assert_eq!(e.project.artwork.as_ref().unwrap().bytes, vec![1, 2, 3]);
        e.checkpoint();
        e.set_artwork_offset(12.0, -3.0);
        e.edit(|p| p.artwork.as_mut().unwrap().scale_percent = 150.0);
        e.undo();
        assert_eq!(e.project.artwork.as_ref().unwrap().scale_percent, 100.0);
        assert_eq!(e.project.artwork.as_ref().unwrap().offset_x_mm, 12.0);
        e.undo();
        assert!(e.project.artwork.as_ref().unwrap().is_centered());
        e.redo();
        assert_eq!(e.project.artwork.as_ref().unwrap().offset_y_mm, -3.0);
        assert_eq!(e.project.artwork.as_ref().unwrap().bytes, vec![1, 2, 3]);
    }

    #[test]
    fn arrangement_changes_preserve_first_cell_anchors_and_support_undo() {
        let mut e = Editor::default();
        let original = e.project.layout_sheet(0).unwrap()[0].clone();
        e.set_arrangement(Arrangement::Free);
        let free = &e.project.layout_sheet(0).unwrap()[0];
        assert_eq!((free.x_mm, free.y_mm), (original.x_mm, original.y_mm));
        e.undo();
        assert_eq!(e.project.arrangement, Arrangement::Grid);
        e.redo();
        assert_eq!(e.project.arrangement, Arrangement::Free);
        e.edit_field(|f| f.x_mm = 200.0);
        e.set_arrangement(Arrangement::Grid);
        e.project.validate().unwrap();
        assert_eq!(e.project.fields[0].x_mm, e.project.cell_size().0);
    }

    #[test]
    fn duplicate_delete_and_history_are_consistent() {
        let mut e = Editor::default();
        e.add_field();
        assert_eq!(e.project.fields.len(), 2);
        assert_ne!(e.project.fields[0].id, e.project.fields[1].id);
        e.delete_field();
        assert_eq!(e.project.fields.len(), 1);
        e.undo();
        assert_eq!(e.project.fields.len(), 2);
        e.edit_field(|f| f.font_size_pt = 25.0);
        assert!(!e.can_redo());
        for _ in 0..60 {
            e.edit(|p| p.sequence.start += 1);
        }
        assert_eq!(e.past.len(), 50);
        e.replace(Project::default());
        assert!(!e.can_undo());
    }

    #[test]
    fn selection_toggles_bulk_edits_and_history_preserve_membership() {
        let mut e = Editor::default();
        e.add_field();
        e.add_field();
        e.select(1, false);
        e.select(2, true);
        assert_eq!(e.selected_ids, BTreeSet::from([1, 2]));
        e.edit_field(|f| {
            f.font_size_pt = 25.0;
            f.rotation = numbering_core::Rotation::Deg90;
        });
        assert_eq!(e.project.fields[0].font_size_pt, 25.0);
        assert_eq!(e.project.fields[1].font_size_pt, 25.0);
        assert_eq!(e.project.fields[2].font_size_pt, 14.0);
        e.undo();
        assert_eq!(e.selected_ids, BTreeSet::from([1, 2]));
        assert_eq!(e.project.fields[0].font_size_pt, 14.0);
        e.redo();
        assert_eq!(e.project.fields[1].font_size_pt, 25.0);
        e.select(2, true);
        assert_eq!(e.selected_ids, BTreeSet::from([1]));
        assert_eq!(e.selected, 1);
        e.select_all();
        e.delete_field();
        assert_eq!(e.project.fields.len(), 1);
        assert_eq!(e.selected_ids, BTreeSet::from([1]));
    }

    #[test]
    fn group_movement_clamps_once_and_preserves_spacing() {
        let mut e = Editor::default();
        e.set_arrangement(Arrangement::Free);
        e.add_field();
        e.select_all();
        let origins = e.selection_origins();
        let spacing = (origins[1].1 - origins[0].1, origins[1].2 - origins[0].2);
        e.move_selection(1000.0, 1000.0);
        assert_eq!(e.project.fields[1].x_mm, 210.0);
        assert_eq!(e.project.fields[1].y_mm, 297.0);
        assert_eq!(
            (
                e.project.fields[1].x_mm - e.project.fields[0].x_mm,
                e.project.fields[1].y_mm - e.project.fields[0].y_mm
            ),
            spacing
        );
        e.undo();
        assert_eq!(e.selection_origins(), origins);
        e.translate_from(&origins, -1000.0, -1000.0);
        assert_eq!(e.project.fields[0].x_mm, 0.0);
        assert_eq!(e.project.fields[0].y_mm, 0.0);
        assert_eq!(
            (e.project.fields[1].x_mm, e.project.fields[1].y_mm),
            spacing
        );
        e.project.validate().unwrap();
    }

    #[test]
    fn start_numbers_apply_to_every_selected_field_only() {
        let mut e = Editor::default();
        e.add_field();
        e.add_field();
        e.select(1, false);
        e.select(2, true);
        e.edit_field(|f| f.start = Some(500));
        let starts: Vec<_> = e.project.fields.iter().map(|f| f.start).collect();
        assert_eq!(starts, [Some(500), Some(500), None]);
        e.undo();
        assert!(e.project.fields.iter().all(|f| f.start.is_none()));
        e.redo();
        e.select(3, false);
        e.add_field();
        assert_eq!(
            e.project.fields[3].start, None,
            "a copy keeps its source's start"
        );
        e.select(1, false);
        e.add_field();
        assert_eq!(e.project.fields[4].start, Some(500));
        e.select(1, false);
        e.edit_field(|f| f.start = None);
        assert_eq!(e.project.fields[0].start, None);
        e.project.validate().unwrap();
    }

    #[test]
    fn marquee_selection_replaces_extends_and_never_empties() {
        let mut e = Editor::default();
        e.add_field();
        e.add_field();
        e.select(1, false);
        e.set_selection([2, 3]);
        assert_eq!(e.selected_ids, BTreeSet::from([2, 3]));
        assert_eq!(e.selected, 2);
        e.set_selection([]);
        assert_eq!(e.selected_ids, BTreeSet::from([2]));
        e.set_selection([99, 1]);
        assert_eq!(e.selected_ids, BTreeSet::from([1]));
        assert_eq!(e.selected, 1);
        e.set_selection([1, 3, 99]);
        assert_eq!(e.selected_ids, BTreeSet::from([1, 3]));
        assert_eq!(e.revision, 2, "selection changes are not content changes");
    }

    #[test]
    fn font_changes_survive_undo_and_redo_without_history_copies() {
        let mut e = Editor::default();
        e.set_font(Some(numbering_core::CustomFont::new(
            "a.ttf",
            vec![1, 2, 3],
        )));
        e.edit(|p| p.sequence.prefix = "A".into());
        assert!(e.past[0].font.is_none());
        e.undo();
        assert_eq!(e.project.sequence.prefix, "No. ");
        assert_eq!(e.project.font.as_ref().unwrap().name, "a.ttf");
        e.redo();
        assert_eq!(e.project.font.as_ref().unwrap().bytes, vec![1, 2, 3]);
        let revision = e.revision;
        e.set_font(None);
        assert!(e.project.font.is_none());
        assert_eq!(e.revision, revision + 1);
    }
}
