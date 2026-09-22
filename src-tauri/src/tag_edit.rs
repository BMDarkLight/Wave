// Wave
// Copyright (C) 2025 BMDarkLight
//
// Licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).
// See the LICENSE file in the project root for the full license text
// and additional terms (attribution and fork-marking requirements).
// https://github.com/BMDarkLight/Wave

use std::path::Path;

use lofty::config::WriteOptions;
use lofty::file::{AudioFile, TaggedFileExt};
use lofty::picture::{MimeType, Picture, PictureType};
use lofty::prelude::{Accessor, ItemKey};
use lofty::tag::Tag;
use serde::Deserialize;

/// Refuse cover images this large before anything is decoded.
pub const MAX_COVER_BYTES: u64 = 32 * 1024 * 1024;

/// Upper bound for year, track and disc numbers.
const MAX_NUMBER: i32 = 9999;

/// An edit as it arrives from the UI or the CLI.
///
/// Every field is optional, and all of them are text, numbers included. A
/// field that is absent leaves the tag alone; a field set to an empty string
/// clears it. Batch edits lean on that: the dialog sends only the fields the
/// user actually touched, so the rest of each file keeps what it had.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct TagEdit {
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub album_artist: Option<String>,
    pub genre: Option<String>,
    pub year: Option<String>,
    pub track_number: Option<String>,
    pub disc_number: Option<String>,
    pub cover: Option<CoverEdit>,
}

/// What to do with the artwork embedded in the file.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum CoverEdit {
    /// Use the image at this path instead.
    Replace { path: String },
    /// Drop every picture the file carries.
    Remove,
}

/// A field that is being changed, as opposed to one being left alone.
#[derive(Debug, Clone, PartialEq)]
pub enum Change<T> {
    Set(T),
    Clear,
}

/// A [`TagEdit`] with its values validated and its cover image read into
/// memory, ready to apply to any number of files.
#[derive(Debug, Default)]
pub struct ResolvedEdit {
    // Title, artist and album cannot be cleared. Wave falls back to the
    // filename for those, so an empty tag would only look like a bug later.
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub album_artist: Option<Change<String>>,
    pub genre: Option<Change<String>>,
    pub year: Option<Change<i32>>,
    pub track_number: Option<Change<i32>>,
    pub disc_number: Option<Change<i32>>,
    /// JPEG bytes to embed, or `Clear` to strip the artwork.
    pub cover: Option<Change<Vec<u8>>>,
}

impl ResolvedEdit {
    pub fn is_empty(&self) -> bool {
        self.title.is_none()
            && self.artist.is_none()
            && self.album.is_none()
            && self.album_artist.is_none()
            && self.genre.is_none()
            && self.year.is_none()
            && self.track_number.is_none()
            && self.disc_number.is_none()
            && self.cover.is_none()
    }
}

impl TagEdit {
    /// Validate the edit once, up front, so a batch either starts clean or
    /// fails before the first file is touched.
    pub fn resolve(self) -> Result<ResolvedEdit, String> {
        Ok(ResolvedEdit {
            title: required_text("Title", self.title)?,
            artist: required_text("Artist", self.artist)?,
            album: required_text("Album", self.album)?,
            album_artist: optional_text(self.album_artist),
            genre: optional_text(self.genre),
            year: number("Year", self.year)?,
            track_number: number("Track number", self.track_number)?,
            disc_number: number("Disc number", self.disc_number)?,
            cover: self.cover.map(resolve_cover).transpose()?,
        })
    }
}

fn required_text(label: &str, value: Option<String>) -> Result<Option<String>, String> {
    let Some(value) = value else {
        return Ok(None);
    };
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(format!("{label} cannot be empty"));
    }
    Ok(Some(trimmed.to_string()))
}

fn optional_text(value: Option<String>) -> Option<Change<String>> {
    value.map(|value| {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            Change::Clear
        } else {
            Change::Set(trimmed.to_string())
        }
    })
}

fn number(label: &str, value: Option<String>) -> Result<Option<Change<i32>>, String> {
    let Some(value) = value else {
        return Ok(None);
    };
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Ok(Some(Change::Clear));
    }
    let parsed: i32 = trimmed
        .parse()
        .map_err(|_| format!("{label} must be a whole number"))?;
    if !(1..=MAX_NUMBER).contains(&parsed) {
        return Err(format!("{label} must be between 1 and {MAX_NUMBER}"));
    }
    Ok(Some(Change::Set(parsed)))
}

/// Read an image the user picked, with a size ceiling so a stray huge file
/// cannot be pulled into memory whole.
pub fn read_image_file(path: &str) -> Result<Vec<u8>, String> {
    let image_path = Path::new(path);
    if !image_path.is_file() {
        return Err(format!("Image file not found: {path}"));
    }
    let size = std::fs::metadata(image_path)
        .map_err(|e| format!("Failed to read image file: {e}"))?
        .len();
    if size > MAX_COVER_BYTES {
        return Err(format!(
            "Image is too large ({size} bytes, max {MAX_COVER_BYTES})"
        ));
    }
    std::fs::read(image_path).map_err(|e| format!("Failed to read image file: {e}"))
}

fn resolve_cover(cover: CoverEdit) -> Result<Change<Vec<u8>>, String> {
    let CoverEdit::Replace { path } = cover else {
        return Ok(Change::Clear);
    };
    let data = read_image_file(&path)?;
    // Artwork is re-encoded to a bounded JPEG on the way in. A 10 MB PNG
    // dropped onto every track of an album would otherwise multiply across the
    // whole batch, and JPEG is the one picture format every tag container
    // handles well.
    Ok(Change::Set(crate::cover_art::make_embeddable_jpeg(&data)?))
}

/// Write an edit into the file's own tags.
///
/// Fields the edit does not mention are left exactly as they were, so changing
/// a genre never stamps in an artist that Wave only guessed from the filename.
pub fn write_to_file(path: &Path, edit: &ResolvedEdit) -> Result<(), String> {
    if edit.is_empty() {
        return Ok(());
    }

    let mut file = lofty::read_from_path(path)
        .map_err(|e| format!("Failed to read tags from {}: {e}", path.display()))?;

    let tag_type = file.primary_tag_type();
    if file.primary_tag_mut().is_none() {
        // The file may still carry a tag of some other kind, an MP3 with
        // nothing but ID3v1 for instance. Carry that one over instead of
        // writing a second tag beside it and leaving two sets of values.
        let mut tag = file
            .tags()
            .first()
            .cloned()
            .unwrap_or_else(|| Tag::new(tag_type));
        tag.re_map(tag_type);
        file.insert_tag(tag);
    }
    let tag = file
        .primary_tag_mut()
        .ok_or_else(|| format!("{} cannot hold tags", path.display()))?;

    apply(tag, edit);

    file.save_to_path(path, WriteOptions::default())
        .map_err(|e| format!("Failed to write tags to {}: {e}", path.display()))
}

fn apply(tag: &mut Tag, edit: &ResolvedEdit) {
    if let Some(title) = &edit.title {
        tag.set_title(title.clone());
    }
    if let Some(artist) = &edit.artist {
        tag.set_artist(artist.clone());
    }
    if let Some(album) = &edit.album {
        tag.set_album(album.clone());
    }
    match &edit.album_artist {
        Some(Change::Set(value)) => {
            tag.insert_text(ItemKey::AlbumArtist, value.clone());
        }
        Some(Change::Clear) => tag.remove_key(ItemKey::AlbumArtist),
        None => {}
    }
    match &edit.genre {
        Some(Change::Set(value)) => tag.set_genre(value.clone()),
        Some(Change::Clear) => tag.remove_genre(),
        None => {}
    }
    match &edit.year {
        Some(Change::Set(year)) => {
            // A full date already in the file keeps its month and day as long
            // as the year itself did not change.
            let keeps_year = tag
                .get_string(ItemKey::RecordingDate)
                .is_some_and(|date| date.starts_with(&year.to_string()));
            if !keeps_year {
                tag.insert_text(ItemKey::RecordingDate, year.to_string());
            }
        }
        Some(Change::Clear) => {
            tag.remove_key(ItemKey::RecordingDate);
            tag.remove_key(ItemKey::Year);
        }
        None => {}
    }
    match &edit.track_number {
        Some(Change::Set(number)) => tag.set_track(*number as u32),
        Some(Change::Clear) => tag.remove_track(),
        None => {}
    }
    match &edit.disc_number {
        Some(Change::Set(number)) => tag.set_disk(*number as u32),
        Some(Change::Clear) => tag.remove_disk(),
        None => {}
    }
    match &edit.cover {
        Some(Change::Set(jpeg)) => {
            remove_pictures(tag);
            tag.push_picture(
                Picture::unchecked(jpeg.clone())
                    .pic_type(PictureType::CoverFront)
                    .mime_type(MimeType::Jpeg)
                    .build(),
            );
        }
        // Every picture goes, not only the front cover: art filed under some
        // other picture type would keep showing up otherwise.
        Some(Change::Clear) => remove_pictures(tag),
        None => {}
    }
}

fn remove_pictures(tag: &mut Tag) {
    while !tag.pictures().is_empty() {
        tag.remove_picture(0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn temp_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("wave-test-{}-{}", uuid::Uuid::new_v4(), name))
    }

    /// Smallest playable WAV symphonia will probe: a header plus one sample.
    fn write_silent_wav(path: &Path) {
        let samples: [u8; 4] = [0, 0, 0, 0];
        let mut wav = Vec::new();
        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(&(36u32 + samples.len() as u32).to_le_bytes());
        wav.extend_from_slice(b"WAVEfmt ");
        wav.extend_from_slice(&16u32.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes()); // PCM
        wav.extend_from_slice(&1u16.to_le_bytes()); // mono
        wav.extend_from_slice(&44100u32.to_le_bytes());
        wav.extend_from_slice(&88200u32.to_le_bytes()); // byte rate
        wav.extend_from_slice(&2u16.to_le_bytes()); // block align
        wav.extend_from_slice(&16u16.to_le_bytes()); // bits per sample
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&(samples.len() as u32).to_le_bytes());
        wav.extend_from_slice(&samples);
        std::fs::write(path, wav).expect("failed to write test wav");
    }

    fn write_png(path: &Path) {
        let image = image::RgbImage::from_pixel(4, 4, image::Rgb([10, 120, 220]));
        image.save(path).expect("failed to write test png");
    }

    fn read_tag(path: &Path) -> Tag {
        lofty::read_from_path(path)
            .expect("failed to re-read tags")
            .primary_tag()
            .cloned()
            .expect("file carries no tag")
    }

    /// Stripping the last picture can leave the tag empty, and lofty drops an
    /// empty tag on save, so a missing tag counts as no pictures.
    fn picture_count(path: &Path) -> usize {
        lofty::read_from_path(path)
            .expect("failed to re-read tags")
            .primary_tag()
            .map(|tag| tag.pictures().len())
            .unwrap_or(0)
    }

    // ── resolve: text fields ────────────────────────────────────────────────

    #[test]
    fn absent_fields_stay_absent() {
        let edit = TagEdit::default().resolve().unwrap();
        assert!(edit.is_empty());
    }

    #[test]
    fn text_values_are_trimmed() {
        let edit = TagEdit {
            title: Some("  Blue Monday  ".into()),
            ..Default::default()
        }
        .resolve()
        .unwrap();
        assert_eq!(edit.title.as_deref(), Some("Blue Monday"));
    }

    #[test]
    fn blank_title_is_rejected() {
        let error = TagEdit {
            title: Some("   ".into()),
            ..Default::default()
        }
        .resolve()
        .unwrap_err();
        assert_eq!(error, "Title cannot be empty");
    }

    #[test]
    fn blank_genre_clears_the_tag() {
        let edit = TagEdit {
            genre: Some(String::new()),
            ..Default::default()
        }
        .resolve()
        .unwrap();
        assert_eq!(edit.genre, Some(Change::Clear));
    }

    // ── resolve: numbers ────────────────────────────────────────────────────

    #[test]
    fn year_is_parsed() {
        let edit = TagEdit {
            year: Some("1983".into()),
            ..Default::default()
        }
        .resolve()
        .unwrap();
        assert_eq!(edit.year, Some(Change::Set(1983)));
    }

    #[test]
    fn blank_year_clears_the_tag() {
        let edit = TagEdit {
            year: Some("  ".into()),
            ..Default::default()
        }
        .resolve()
        .unwrap();
        assert_eq!(edit.year, Some(Change::Clear));
    }

    #[test]
    fn non_numeric_year_is_rejected() {
        let error = TagEdit {
            year: Some("eighty three".into()),
            ..Default::default()
        }
        .resolve()
        .unwrap_err();
        assert_eq!(error, "Year must be a whole number");
    }

    #[test]
    fn out_of_range_track_number_is_rejected() {
        let error = TagEdit {
            track_number: Some("0".into()),
            ..Default::default()
        }
        .resolve()
        .unwrap_err();
        assert_eq!(error, "Track number must be between 1 and 9999");
    }

    // ── resolve: cover ──────────────────────────────────────────────────────

    #[test]
    fn missing_cover_file_is_rejected() {
        let missing = temp_path("nope.png");
        let error = TagEdit {
            cover: Some(CoverEdit::Replace {
                path: missing.to_string_lossy().into_owned(),
            }),
            ..Default::default()
        }
        .resolve()
        .unwrap_err();
        assert!(error.starts_with("Image file not found:"), "{error}");
    }

    #[test]
    fn picked_cover_becomes_jpeg() {
        let png = temp_path("cover.png");
        write_png(&png);

        let edit = TagEdit {
            cover: Some(CoverEdit::Replace {
                path: png.to_string_lossy().into_owned(),
            }),
            ..Default::default()
        }
        .resolve()
        .unwrap();

        let Some(Change::Set(jpeg)) = edit.cover else {
            panic!("expected cover bytes");
        };
        assert_eq!(&jpeg[..2], &[0xFF, 0xD8], "not a JPEG");
        let _ = std::fs::remove_file(png);
    }

    // ── write_to_file ───────────────────────────────────────────────────────

    #[test]
    fn every_field_lands_in_the_file() {
        let wav = temp_path("tagged.wav");
        write_silent_wav(&wav);

        let edit = TagEdit {
            title: Some("Temporary Secretary".into()),
            artist: Some("Paul McCartney".into()),
            album: Some("McCartney II".into()),
            album_artist: Some("Paul McCartney".into()),
            genre: Some("Electronic".into()),
            year: Some("1980".into()),
            track_number: Some("3".into()),
            disc_number: Some("1".into()),
            cover: None,
        }
        .resolve()
        .unwrap();
        write_to_file(&wav, &edit).unwrap();

        let tag = read_tag(&wav);
        assert_eq!(tag.title().as_deref(), Some("Temporary Secretary"));
        assert_eq!(tag.artist().as_deref(), Some("Paul McCartney"));
        assert_eq!(tag.album().as_deref(), Some("McCartney II"));
        assert_eq!(tag.genre().as_deref(), Some("Electronic"));
        assert_eq!(tag.get_string(ItemKey::RecordingDate), Some("1980"));
        assert_eq!(tag.track(), Some(3));

        let _ = std::fs::remove_file(wav);
    }

    #[test]
    fn untouched_fields_survive_a_later_edit() {
        let wav = temp_path("partial.wav");
        write_silent_wav(&wav);

        let first = TagEdit {
            title: Some("Girls & Boys".into()),
            artist: Some("Blur".into()),
            ..Default::default()
        }
        .resolve()
        .unwrap();
        write_to_file(&wav, &first).unwrap();

        let second = TagEdit {
            genre: Some("Britpop".into()),
            ..Default::default()
        }
        .resolve()
        .unwrap();
        write_to_file(&wav, &second).unwrap();

        let tag = read_tag(&wav);
        assert_eq!(tag.title().as_deref(), Some("Girls & Boys"));
        assert_eq!(tag.artist().as_deref(), Some("Blur"));
        assert_eq!(tag.genre().as_deref(), Some("Britpop"));

        let _ = std::fs::remove_file(wav);
    }

    #[test]
    fn clearing_a_field_removes_it_from_the_file() {
        let wav = temp_path("cleared.wav");
        write_silent_wav(&wav);

        let written = TagEdit {
            title: Some("Song 2".into()),
            genre: Some("Britpop".into()),
            year: Some("1997".into()),
            ..Default::default()
        }
        .resolve()
        .unwrap();
        write_to_file(&wav, &written).unwrap();

        let cleared = TagEdit {
            genre: Some(String::new()),
            year: Some(String::new()),
            ..Default::default()
        }
        .resolve()
        .unwrap();
        write_to_file(&wav, &cleared).unwrap();

        let tag = read_tag(&wav);
        assert_eq!(tag.title().as_deref(), Some("Song 2"));
        assert_eq!(tag.genre(), None);
        assert_eq!(tag.get_string(ItemKey::RecordingDate), None);

        let _ = std::fs::remove_file(wav);
    }

    #[test]
    fn a_full_date_keeps_its_month_and_day_when_the_year_is_unchanged() {
        let wav = temp_path("dated.wav");
        write_silent_wav(&wav);

        let mut file = lofty::read_from_path(&wav).unwrap();
        let tag_type = file.primary_tag_type();
        let mut tag = Tag::new(tag_type);
        tag.insert_text(ItemKey::RecordingDate, "1980-05-16".into());
        file.insert_tag(tag);
        file.save_to_path(&wav, WriteOptions::default()).unwrap();

        let edit = TagEdit {
            year: Some("1980".into()),
            ..Default::default()
        }
        .resolve()
        .unwrap();
        write_to_file(&wav, &edit).unwrap();

        assert_eq!(
            read_tag(&wav).get_string(ItemKey::RecordingDate),
            Some("1980-05-16")
        );

        let _ = std::fs::remove_file(wav);
    }

    #[test]
    fn cover_art_is_replaced_rather_than_stacked() {
        let wav = temp_path("art.wav");
        write_silent_wav(&wav);
        let png = temp_path("art.png");
        write_png(&png);

        let edit = || {
            TagEdit {
                cover: Some(CoverEdit::Replace {
                    path: png.to_string_lossy().into_owned(),
                }),
                ..Default::default()
            }
            .resolve()
            .unwrap()
        };
        write_to_file(&wav, &edit()).unwrap();
        write_to_file(&wav, &edit()).unwrap();
        assert_eq!(picture_count(&wav), 1);

        let removal = TagEdit {
            cover: Some(CoverEdit::Remove),
            ..Default::default()
        }
        .resolve()
        .unwrap();
        write_to_file(&wav, &removal).unwrap();
        assert_eq!(picture_count(&wav), 0);

        let _ = std::fs::remove_file(wav);
        let _ = std::fs::remove_file(png);
    }

    #[test]
    fn an_empty_edit_leaves_the_file_alone() {
        let wav = temp_path("untouched.wav");
        write_silent_wav(&wav);
        let before = std::fs::read(&wav).unwrap();

        write_to_file(&wav, &ResolvedEdit::default()).unwrap();

        assert_eq!(std::fs::read(&wav).unwrap(), before);
        let _ = std::fs::remove_file(wav);
    }
}
