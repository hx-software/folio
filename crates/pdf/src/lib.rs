//! Pure-Rust PDF import, vector export, and raster preview.
use krilla::{
    Document, SerializeSettings,
    color::rgb,
    geom::{Point, Size, Transform},
    page::PageSettings,
    paint::Fill,
    text::{Font, TextDirection},
};
use numbering_core::{
    Alignment, ArtworkKind, Error, MAX_EXPORT_SHEETS, PT_PER_MM, Project, Result, parse_color,
};
use serde::{Deserialize, Serialize};
use std::{io::Cursor, ops::Range, sync::Arc};

pub const FONT_BYTES: &[u8] = include_bytes!("../../../public/NotoSans-Regular.ttf");
const MAX_PREVIEW_PIXELS: f64 = 4_000_000.0;
const MAX_IMAGE_PIXELS: u64 = 40_000_000;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ArtworkInfo {
    pub width_mm: f64,
    pub height_mm: f64,
    pub page_count: usize,
    pub requires_pdf_20: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Preview {
    pub png: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

fn err(message: impl ToString) -> Error {
    Error(message.to_string())
}

fn load_pdf(bytes: &[u8]) -> Result<hayro::hayro_syntax::Pdf> {
    hayro::hayro_syntax::Pdf::new(bytes.to_vec()).map_err(|e| {
        err(format!(
            "Cannot open PDF (encrypted PDFs are not supported): {e:?}"
        ))
    })
}

fn image_dimensions(bytes: &[u8]) -> Result<(u32, u32)> {
    let reader = image::ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(err)?;
    let (w, h) = reader.into_dimensions().map_err(err)?;
    if u64::from(w) * u64::from(h) > MAX_IMAGE_PIXELS || w == 0 || h == 0 {
        return Err(err("Image exceeds the 40 megapixel limit."));
    }
    Ok((w, h))
}

pub fn inspect_artwork(project: &Project) -> Result<ArtworkInfo> {
    let art = project
        .artwork
        .as_ref()
        .ok_or_else(|| err("No artwork loaded."))?;
    if art.bytes.is_empty() || art.bytes.len() > numbering_core::MAX_ARTWORK_BYTES {
        return Err(err("Artwork must be between 1 byte and 20 MB."));
    }
    match art.kind {
        ArtworkKind::Pdf => {
            let pdf = load_pdf(&art.bytes)?;
            let page = pdf
                .pages()
                .get(art.page_index)
                .ok_or_else(|| err("Artwork page is out of range."))?;
            let (w, h) = page.render_dimensions();
            for dimension in [w as f64 / PT_PER_MM, h as f64 / PT_PER_MM] {
                if !dimension.is_finite() || !(10.0..=2000.0).contains(&dimension) {
                    return Err(err(
                        "PDF artwork pages must be between 10 and 2000 mm across.",
                    ));
                }
            }
            Ok(ArtworkInfo {
                width_mm: w as f64 / PT_PER_MM,
                height_mm: h as f64 / PT_PER_MM,
                page_count: pdf.pages().len(),
                requires_pdf_20: matches!(pdf.version(), hayro::hayro_syntax::PdfVersion::Pdf20),
            })
        }
        _ => {
            let (w, h) = image_dimensions(&art.bytes)?;
            Ok(ArtworkInfo {
                width_mm: w as f64 * 25.4 / 96.0,
                height_mm: h as f64 * 25.4 / 96.0,
                page_count: 1,
                requires_pdf_20: false,
            })
        }
    }
}

fn png_preview(image: image::RgbaImage) -> Result<Preview> {
    let (width, height) = image.dimensions();
    let mut png = Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(image)
        .write_to(&mut png, image::ImageFormat::Png)
        .map_err(err)?;
    Ok(Preview {
        png: png.into_inner(),
        width,
        height,
    })
}

pub fn render_pdf(bytes: &[u8], page_index: usize, target_width: u32) -> Result<Preview> {
    use hayro::vello_cpu::{color::palette::css::WHITE, peniko::ImageAlphaType};
    use hayro::{
        PixmapSettings, RenderCache, RenderSettings, hayro_interpret::InterpreterSettings,
    };
    let pdf = load_pdf(bytes)?;
    let page = pdf
        .pages()
        .get(page_index)
        .ok_or_else(|| err("Preview page is out of range."))?;
    let (w, h) = page.render_dimensions();
    if !w.is_finite() || !h.is_finite() || w <= 0.0 || h <= 0.0 {
        return Err(err("PDF page dimensions are invalid."));
    }
    let scale = (target_width.clamp(100, 1600) as f64 / w as f64)
        .min((MAX_PREVIEW_PIXELS / (w as f64 * h as f64)).sqrt()) as f32;
    let pixmap = hayro::render(
        page,
        &RenderCache::new(),
        &InterpreterSettings::default(),
        &RenderSettings::default(),
        &PixmapSettings {
            x_scale: scale,
            y_scale: scale,
            bg_color: WHITE,
        },
    );
    let image = image::RgbaImage::from_raw(
        pixmap.width() as u32,
        pixmap.height() as u32,
        pixmap.take_rgba8(ImageAlphaType::Alpha),
    )
    .ok_or_else(|| err("Invalid rendered pixels."))?;
    png_preview(image)
}

pub fn background(project: &Project, target_width: u32) -> Result<Preview> {
    let artwork = project
        .artwork
        .as_ref()
        .ok_or_else(|| err("No artwork loaded."))?;
    inspect_artwork(project)?;
    match artwork.kind {
        ArtworkKind::Pdf => render_pdf(&artwork.bytes, artwork.page_index, target_width),
        _ => {
            let decoded = image::load_from_memory(&artwork.bytes).map_err(err)?;
            let ratio = decoded.height() as f64 / decoded.width() as f64;
            let width = (target_width.clamp(100, 1600) as f64)
                .min((MAX_PREVIEW_PIXELS / ratio).sqrt()) as u32;
            png_preview(
                decoded
                    .thumbnail(width, (width as f64 * ratio).max(1.0) as u32)
                    .into_rgba8(),
            )
        }
    }
}

/// Full-sheet artwork is fitted proportionally and centred. It is never stretched.
pub fn fit_artwork(project: &Project, info: &ArtworkInfo) -> (f64, f64, f64, f64) {
    let scale =
        (project.page.width_mm / info.width_mm).min(project.page.height_mm / info.height_mm);
    let w = info.width_mm * scale;
    let h = info.height_mm * scale;
    (
        (project.page.width_mm - w) / 2.0,
        (project.page.height_mm - h) / 2.0,
        w,
        h,
    )
}

pub fn export(project: &Project, sheets: Range<usize>) -> Result<Vec<u8>> {
    export_with_progress(project, sheets, |_, _| {})
}

pub fn export_with_progress(
    project: &Project,
    sheets: Range<usize>,
    mut progress: impl FnMut(usize, usize),
) -> Result<Vec<u8>> {
    project.validate()?;
    if sheets.is_empty() || sheets.end > project.sheet_count() || sheets.len() > MAX_EXPORT_SHEETS {
        return Err(err("Choose a valid export range of at most 500 sheets."));
    }
    if sheets.len() * project.slots() * project.fields.len()
        > numbering_core::MAX_NUMBERS_PER_EXPORT
    {
        return Err(err(
            "Too many number positions in one export. Reduce the sheet range to stay under 200,000 positions.",
        ));
    }
    let font =
        Font::new(FONT_BYTES.to_vec().into(), 0).ok_or_else(|| err("Bundled font is invalid."))?;
    let face =
        rustybuzz::Face::from_slice(FONT_BYTES, 0).ok_or_else(|| err("Cannot measure font."))?;
    let artwork_info = project
        .artwork
        .as_ref()
        .map(|_| inspect_artwork(project))
        .transpose()?;
    let embedded_pdf = project
        .artwork
        .as_ref()
        .filter(|a| a.kind == ArtworkKind::Pdf)
        .map(|a| {
            // Krilla's released PDF API uses its own compatible hayro-syntax version.
            krilla::pdf::Pdf::new(a.bytes.clone())
                .map(|pdf| krilla::pdf::PdfDocument::new(Arc::new(pdf)))
                .map_err(|e| err(format!("Cannot embed PDF: {e:?}")))
        })
        .transpose()?;
    let embedded_image = project
        .artwork
        .as_ref()
        .filter(|a| a.kind != ArtworkKind::Pdf)
        .map(|a| {
            match a.kind {
                ArtworkKind::Png => krilla::image::Image::from_png(a.bytes.clone().into(), true),
                ArtworkKind::Jpeg => krilla::image::Image::from_jpeg(a.bytes.clone().into(), true),
                ArtworkKind::Pdf => unreachable!(),
            }
            .map_err(err)
        })
        .transpose()?;
    let configuration = krilla::configure::ConfigurationBuilder::new()
        .with_version(
            if artwork_info.as_ref().is_some_and(|i| i.requires_pdf_20) {
                krilla::configure::PdfVersion::Pdf20
            } else {
                krilla::configure::PdfVersion::Pdf17
            },
        )
        .finish()
        .map_err(|e| err(format!("{e:?}")))?;
    let mut document = Document::new_with(SerializeSettings {
        configuration,
        ..Default::default()
    });
    let pt = |mm: f64| (mm * PT_PER_MM) as f32;
    let total = sheets.len();
    for (index, sheet) in sheets.enumerate() {
        let mut page = document.start_page_with(
            PageSettings::from_wh(pt(project.page.width_mm), pt(project.page.height_mm)).unwrap(),
        );
        let mut surface = page.surface();
        if let Some(info) = &artwork_info {
            let (x, y, w, h) = fit_artwork(project, info);
            surface.push_transform(&Transform::from_translate(pt(x), pt(y)));
            let size = Size::from_wh(pt(w), pt(h)).unwrap();
            if let Some(pdf) = &embedded_pdf {
                surface.draw_pdf_page(pdf, size, project.artwork.as_ref().unwrap().page_index);
            }
            if let Some(image) = &embedded_image {
                surface.draw_image(image.clone(), size);
            }
            surface.pop();
        }
        for number in project.layout_sheet(sheet)? {
            let mut buffer = rustybuzz::UnicodeBuffer::new();
            buffer.push_str(&number.text);
            buffer.guess_segment_properties();
            let shaped = rustybuzz::shape(&face, &[], buffer);
            if shaped.glyph_infos().iter().any(|g| g.glyph_id == 0) {
                return Err(err(
                    "The bundled Noto Sans font does not support a character in your prefix or suffix.",
                ));
            }
            let width = shaped
                .glyph_positions()
                .iter()
                .map(|g| g.x_advance as f64)
                .sum::<f64>()
                / face.units_per_em() as f64
                * number.field.font_size_pt;
            let offset = match number.field.alignment {
                Alignment::Left => 0.0,
                Alignment::Center => -width / 2.0,
                Alignment::Right => -width,
            };
            let [r, g, b] = parse_color(&number.field.color).unwrap();
            surface.set_fill(Some(Fill {
                paint: rgb::Color::new(r, g, b).into(),
                ..Default::default()
            }));
            let radians = (number.field.rotation.degrees() as f32).to_radians();
            let (sin, cos) = radians.sin_cos();
            surface.push_transform(&Transform::from_row(
                cos,
                sin,
                -sin,
                cos,
                pt(number.x_mm),
                pt(number.y_mm),
            ));
            surface.draw_text(
                Point::from_xy(offset as f32, 0.0),
                font.clone(),
                number.field.font_size_pt as f32,
                &number.text,
                false,
                TextDirection::LeftToRight,
            );
            surface.pop();
        }
        surface.finish();
        page.finish();
        progress(index + 1, total);
    }
    document
        .finish()
        .map_err(|e| err(format!("PDF export failed: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn export_and_render_generated_pdf() {
        let mut p = Project::default();
        p.sequence.quantity = 8;
        let bytes = export(&p, 0..2).unwrap();
        assert!(bytes.starts_with(b"%PDF-"));
        let pdf = load_pdf(&bytes).unwrap();
        assert_eq!(pdf.pages().len(), 2);
        let (w, h) = pdf.pages()[0].render_dimensions();
        assert!((w as f64 - 210.0 * PT_PER_MM).abs() < 0.01);
        assert!((h as f64 - 297.0 * PT_PER_MM).abs() < 0.01);
        let preview = render_pdf(&bytes, 0, 400).unwrap();
        assert!(preview.png.starts_with(b"\x89PNG"));
        assert!((399..=401).contains(&preview.width));
        assert!(preview.height > preview.width);
    }

    #[test]
    fn embeds_pdf_artwork_and_keeps_numbers() {
        let source = Project::default();
        let bytes = export(&source, 0..1).unwrap();
        let mut p = Project::default();
        p.artwork = Some(numbering_core::Artwork {
            name: "fixture.pdf".into(),
            kind: ArtworkKind::Pdf,
            bytes,
            page_index: 0,
        });
        let info = inspect_artwork(&p).unwrap();
        assert_eq!(info.page_count, 1);
        let result = export(&p, 0..2).unwrap();
        assert_eq!(load_pdf(&result).unwrap().pages().len(), 2);
        assert!(!render_pdf(&result, 1, 200).unwrap().png.is_empty());
    }

    #[test]
    fn rejects_invalid_range_and_artwork() {
        let mut p = Project::default();
        assert!(export(&p, 0..0).is_err());
        assert!(export(&p, 0..999).is_err());
        assert!(render_pdf(b"not a pdf", 0, 400).is_err());
        p.artwork = Some(numbering_core::Artwork {
            name: "bad.pdf".into(),
            kind: ArtworkKind::Pdf,
            bytes: b"broken".to_vec(),
            page_index: 0,
        });
        assert!(inspect_artwork(&p).is_err());
    }

    #[test]
    fn rotated_cropped_artwork_retains_its_appearance() {
        use pdf_writer::{Content, Pdf, Rect, Ref};
        let mut source = Pdf::new();
        source.catalog(Ref::new(1)).pages(Ref::new(2));
        source.pages(Ref::new(2)).kids([Ref::new(3)]).count(1);
        source
            .page(Ref::new(3))
            .parent(Ref::new(2))
            .media_box(Rect::new(0.0, 0.0, 300.0, 400.0))
            .crop_box(Rect::new(30.0, 40.0, 270.0, 360.0))
            .rotate(90)
            .contents(Ref::new(4));
        let mut content = Content::new();
        content
            .set_fill_rgb(0.8, 0.1, 0.2)
            .rect(80.0, 90.0, 70.0, 110.0)
            .fill_nonzero();
        source.stream(Ref::new(4), &content.finish());
        let bytes = source.finish();
        let original = render_pdf(&bytes, 0, 640).unwrap();
        let mut p = Project::default();
        p.artwork = Some(numbering_core::Artwork {
            name: "cropped.pdf".into(),
            kind: ArtworkKind::Pdf,
            bytes,
            page_index: 0,
        });
        let info = inspect_artwork(&p).unwrap();
        p.page.width_mm = info.width_mm;
        p.page.height_mm = info.height_mm;
        p.grid.columns = 1;
        p.grid.rows = 1;
        p.fields[0].color = "#ffffff".into();
        p.fields[0].x_mm = 0.0;
        p.fields[0].y_mm = 2.0; // Invisible white numbering stays outside the coloured artwork.
        let exported = export(&p, 0..1).unwrap();
        let preview = render_pdf(&exported, 0, 640).unwrap();
        if let Ok(directory) = std::env::var("FOLIO_TEST_ARTIFACTS") {
            std::fs::write(
                std::path::Path::new(&directory).join("original.png"),
                &original.png,
            )
            .unwrap();
            std::fs::write(
                std::path::Path::new(&directory).join("embedded.png"),
                &preview.png,
            )
            .unwrap();
        }
        assert_eq!(
            (original.width, original.height),
            (preview.width, preview.height)
        );
        let original = image::load_from_memory(&original.png).unwrap().into_rgba8();
        let output = image::load_from_memory(&preview.png).unwrap().into_rgba8();
        let changed = original
            .pixels()
            .zip(output.pixels())
            .filter(|(a, b)| a.0.iter().zip(b.0.iter()).any(|(a, b)| a.abs_diff(*b) > 4))
            .count();
        assert!(
            changed < 500,
            "Artwork changed at {changed} pixels after embedding"
        );
    }

    #[test]
    fn repeated_artwork_is_reused_and_export_progress_is_reported() {
        let bytes = export(&Project::default(), 0..1).unwrap();
        let mut p = Project::default();
        p.sequence.quantity = 600;
        p.artwork = Some(numbering_core::Artwork {
            name: "source.pdf".into(),
            kind: ArtworkKind::Pdf,
            bytes,
            page_index: 0,
        });
        let one = export(&p, 0..1).unwrap();
        let mut progress = Vec::new();
        let many = export_with_progress(&p, 0..100, |completed, total| {
            progress.push((completed, total))
        })
        .unwrap();
        assert_eq!(progress.first(), Some(&(1, 100)));
        assert_eq!(progress.last(), Some(&(100, 100)));
        assert!(
            many.len() < one.len() * 20,
            "Repeated artwork appears to be duplicated"
        );
    }

    #[test]
    fn accepts_images_and_rejects_missing_glyphs() {
        for format in [image::ImageFormat::Png, image::ImageFormat::Jpeg] {
            let image = image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(
                64,
                32,
                image::Rgb([200, 150, 100]),
            ));
            let mut bytes = Cursor::new(Vec::new());
            image.write_to(&mut bytes, format).unwrap();
            let mut p = Project::default();
            p.artwork = Some(numbering_core::Artwork {
                name: "image".into(),
                kind: if format == image::ImageFormat::Png {
                    ArtworkKind::Png
                } else {
                    ArtworkKind::Jpeg
                },
                bytes: bytes.into_inner(),
                page_index: 0,
            });
            assert!(!background(&p, 200).unwrap().png.is_empty());
            assert!(!export(&p, 0..1).unwrap().is_empty());
        }
        let mut p = Project::default();
        p.sequence.prefix = "🔥".into();
        assert!(
            export(&p, 0..1)
                .unwrap_err()
                .to_string()
                .contains("does not support")
        );
    }
}
