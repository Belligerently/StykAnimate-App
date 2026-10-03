//! Audio tracks and clips (src/editor-audio.mjs): track and clip settings changed, tracks
//! and clips deleted, each one `audio` history entry that touches no frame or figure
//! (`publish`). Commands that make new identities (adding, importing, splitting,
//! duplicating, pasting) and ripples with frame edits stay in JavaScript.

use std::cell::RefCell;
use std::sync::{Arc, Weak};

use styk_doc::{Obj, Value};

use crate::history::{Command, CommandKind, Direction};
use crate::{EditError, Editor};

/// `AUDIO_RATE`.
const RATE: f64 = 48000.0;
const CHUNK_BYTES: f64 = 192.0 * 1024.0;
const MAX_FILE_BYTES: f64 = 32.0 * 1024.0 * 1024.0;
const MAX_PROJECT_BYTES: f64 = 96.0 * 1024.0 * 1024.0;
const MAX_SECONDS: f64 = 3600.0;
const MAX_TRACKS: usize = 64;
const MAX_CLIPS: usize = 4096;
const MAX_VOICES: i64 = 32;

fn invalid(key: &str) -> EditError {
    EditError::Invalid(format!("errors.audioModel.{key}"))
}

/// `Number.isFinite(value)`: a number, not a coerced one.
fn finite(value: Option<&Value>) -> Option<f64> {
    match value {
        Some(Value::Num(n)) if n.is_finite() => Some(*n),
        _ => None,
    }
}

/// `number(value, min, max, name)`.
fn number(value: Option<&Value>, min: f64, max: f64) -> Result<f64, EditError> {
    match finite(value) {
        Some(n) if n >= min && n <= max => Ok(n),
        _ => Err(invalid("mustBeBetweenAnd")),
    }
}

/// `id(value)`: 1 to 100 letters, digits, `_` or `-`.
fn identity_text(value: Option<&Value>) -> Option<&str> {
    value.and_then(Value::as_str).filter(|text| (1..=100).contains(&text.len()) && text.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-'))
}

/// White space as `String.prototype.trim` removes it.
fn js_space(c: char) -> bool {
    matches!(c, '\t' | '\n' | '\u{b}' | '\u{c}' | '\r' | ' ' | '\u{a0}' | '\u{1680}' | '\u{2000}'..='\u{200a}' | '\u{2028}' | '\u{2029}' | '\u{202f}' | '\u{205f}' | '\u{3000}' | '\u{feff}')
}

/// A name as the model accepts it: text that is not only white space, at most `max`
/// UTF-16 units long.
fn name_ok(value: Option<&Value>, max: usize) -> bool {
    value.and_then(Value::as_str).is_some_and(|text| !text.chars().all(js_space) && text.encode_utf16().count() <= max)
}

/// A strict-equality check (`===`) of two fields: containers by identity.
fn strict_eq(a: Option<&Value>, b: Option<&Value>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(Value::Num(x)), Some(Value::Num(y))) => x == y,
        (Some(Value::Bool(x)), Some(Value::Bool(y))) => x == y,
        (Some(Value::Null), Some(Value::Null)) => true,
        (Some(x @ (Value::Str(_) | Value::Blob(_))), Some(y @ (Value::Str(_) | Value::Blob(_)))) => x.text() == y.text(),
        (Some(x), Some(y)) => x.same_node(y),
        _ => false,
    }
}

thread_local! {
    /// Recordings already checked, by identity (`checkedAssets`, a `WeakSet` there): a
    /// recording is checked once, not at every edit, and not kept alive by being checked.
    static CHECKED: RefCell<Vec<Weak<Obj>>> = const { RefCell::new(Vec::new()) };
}

/// `validateAudioAsset(asset)`; returns its size.
fn validate_asset(asset: &Value) -> Result<f64, EditError> {
    let Value::Obj(object) = asset else { return Err(invalid("invalidAudioAsset")) };
    if CHECKED.with(|checked| checked.borrow().iter().any(|known| std::ptr::eq(known.as_ptr(), Arc::as_ptr(object)))) {
        return Ok(finite(asset.get("size")).unwrap_or(f64::NAN));
    }
    if identity_text(asset.get("id")).is_none() || !name_ok(asset.get("name"), 240) {
        return Err(invalid("invalidAudioAssetNameOrIdentity"));
    }
    let size = number(asset.get("size"), 1.0, MAX_FILE_BYTES)?;
    number(asset.get("duration"), 1.0 / RATE, MAX_SECONDS)?;
    let chunks = asset.get("chunks").and_then(Value::elements).map(|e| e.iter().collect::<Vec<_>>());
    let Some(chunks) = chunks.filter(|c| size.fract() == 0.0 && c.len() as f64 <= (MAX_FILE_BYTES / CHUNK_BYTES).ceil()) else {
        return Err(invalid("invalidEmbeddedAudio"));
    };
    let mut total = 0.0;
    for chunk in &chunks {
        total += chunk_bytes(chunk.get("data_base64")).ok_or_else(|| invalid("invalidEmbeddedAudioBytes"))?;
    }
    if total != size {
        return Err(invalid("embeddedAudioIsIncomplete"));
    }
    let peaks = asset.get("peaks").and_then(Value::elements).map(|e| e.iter().collect::<Vec<_>>());
    if !peaks.is_some_and(|p| !p.is_empty() && p.len() <= 4096 && p.iter().all(|v| finite(Some(v)).is_some_and(|n| (0.0..=1.0).contains(&n)))) {
        return Err(invalid("invalidAudioWaveform"));
    }
    CHECKED.with(|checked| {
        let mut checked = checked.borrow_mut();
        checked.retain(|known| known.strong_count() > 0);
        checked.push(Arc::downgrade(object));
    });
    Ok(size)
}

/// The bytes one chunk's base64 holds, when it is well formed: non-empty, padded,
/// at most one chunk long.
fn chunk_bytes(value: Option<&Value>) -> Option<f64> {
    let text = match value? {
        // A blob is the canonical base64 of its bytes after its prefix.
        Value::Blob(blob) if blob.prefix.is_empty() => {
            let length = blob.bytes.len().div_ceil(3) * 4;
            return (length > 0 && length as f64 <= CHUNK_BYTES * 4.0 / 3.0).then_some(blob.bytes.len() as f64);
        }
        other @ (Value::Str(_) | Value::Blob(_)) => other.text()?.into_owned(),
        _ => return None,
    };
    let body = text.trim_end_matches('=');
    let padding = text.len() - body.len();
    let well_formed = !text.is_empty()
        && text.len() as f64 <= CHUNK_BYTES * 4.0 / 3.0
        && text.len() % 4 == 0
        && !body.is_empty()
        && padding <= 2
        && body.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'+' || b == b'/');
    well_formed.then(|| (text.len() / 4 * 3 - padding) as f64)
}

/// `validateAudioState(state)`.
fn validate_state(state: &Value) -> Result<(), EditError> {
    let assets = state.get("assets").and_then(Value::as_obj);
    let tracks = state.get("tracks").and_then(Value::elements).map(|e| e.iter().collect::<Vec<_>>());
    let (Some(assets), Some(tracks)) = (assets, tracks) else { return Err(invalid("unsupportedAudioDocument")) };
    if !matches!(state.get("version"), Some(Value::Num(v)) if *v == 1.0) || tracks.len() > MAX_TRACKS {
        return Err(invalid("unsupportedAudioDocument"));
    }
    let mut ids = std::collections::HashSet::new();
    let mut identity = |value: Option<&Value>| -> Result<(), EditError> {
        match identity_text(value) {
            Some(text) if ids.insert(text.to_string()) => Ok(()),
            _ => Err(invalid("duplicateOrInvalidAudioIdentity")),
        }
    };
    let mut bytes = 0.0;
    for (key, asset) in assets.iter() {
        if asset.is_null() {
            return Err(EditError::Unported("an audio recording that is null".into()));
        }
        if !matches!(asset.get("id"), Some(id) if id.as_str() == Some(&**key)) {
            return Err(invalid("invalidAudioAssetReference"));
        }
        identity(Some(&Value::Str(key.clone())))?;
        bytes += validate_asset(asset)?;
    }
    if bytes > MAX_PROJECT_BYTES {
        return Err(invalid("embeddedAudioExceedsThe96MibProjectLimit"));
    }
    let mut count = 0;
    for track in &tracks {
        // `track.id` of null throws a TypeError, not the model's message.
        if track.is_null() {
            return Err(EditError::Unported("an audio track that is null".into()));
        }
        identity(track.get("id"))?;
        let clips = track.get("clips").and_then(Value::elements).map(|e| e.iter().collect::<Vec<_>>());
        let flags = matches!(track.get("mute"), Some(Value::Bool(_))) && matches!(track.get("solo"), Some(Value::Bool(_)));
        let (Some(clips), true, true) = (clips, name_ok(track.get("name"), 120), flags) else {
            return Err(invalid("invalidAudioTrack"));
        };
        number(track.get("gain"), 0.0, 4.0)?;
        for clip in clips {
            count += 1;
            if count > MAX_CLIPS {
                return Err(invalid("aProjectSupportsUpTo4096Audio"));
            }
            if clip.is_null() {
                return Err(EditError::Unported("an audio clip that is null".into()));
            }
            identity(clip.get("id"))?;
            let asset = match clip.get("assetId") {
                Some(Value::Str(id)) => assets.get(id),
                None => None,
                Some(_) => return Err(EditError::Unported("an audio clip whose recording is not named by text".into())),
            };
            let Some(duration) = asset.and_then(|a| finite(a.get("duration"))) else {
                return Err(invalid("audioClipReferencesAMissingRecording"));
            };
            let start = number(clip.get("start"), 0.0, MAX_SECONDS)?;
            let offset = number(clip.get("offset"), 0.0, duration)?;
            let length = number(clip.get("duration"), 1.0 / RATE, duration)?;
            if offset + length > duration + 1e-7 || start + length > MAX_SECONDS {
                return Err(invalid("audioClipExtendsBeyondItsSourceOrThe"));
            }
            number(clip.get("gain"), 0.0, 4.0)?;
            number(clip.get("fadeIn"), 0.0, length)?;
            number(clip.get("fadeOut"), 0.0, length)?;
        }
    }
    // `audibleClips`: no more than 32 sound at once.
    let truthy = |track: &Value, key: &str| track.get(key).is_some_and(Value::truthy);
    let solo = tracks.iter().any(|track| truthy(track, "solo"));
    let mut boundaries: Vec<(f64, i64)> = Vec::new();
    for track in tracks.iter().filter(|t| !truthy(t, "mute") && (!solo || truthy(t, "solo")) && finite(t.get("gain")).is_some_and(|g| g > 0.0)) {
        for clip in track.get("clips").and_then(Value::elements).map(|e| e.iter().collect::<Vec<_>>()).unwrap_or_default() {
            if finite(clip.get("gain")).is_some_and(|g| g > 0.0) {
                let start = finite(clip.get("start")).unwrap_or(0.0);
                boundaries.push((start, 1));
                boundaries.push((start + finite(clip.get("duration")).unwrap_or(0.0), -1));
            }
        }
    }
    boundaries.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal).then(a.1.cmp(&b.1)));
    let mut voices = 0;
    for (_, delta) in boundaries {
        voices += delta;
        if voices > MAX_VOICES {
            return Err(invalid("upTo32AudioClipsCanSoundAt"));
        }
    }
    Ok(())
}

/// `EMPTY_AUDIO`.
fn empty_audio() -> Value {
    let mut state = Obj::new();
    state.set("version", Value::Num(1.0));
    state.set("assets", Value::object(Obj::new()));
    state.set("tracks", Value::array(Vec::new()));
    Value::object(state)
}

/// A copy of `object` with `key` set (`{...object, key: value}`).
fn with(object: &Value, key: &str, value: Value) -> Value {
    let mut copy = object.as_obj().cloned().unwrap_or_default();
    copy.set(key, value);
    Value::object(copy)
}

fn tracks_of(state: &Value) -> Result<Vec<Value>, EditError> {
    state.get("tracks").and_then(Value::elements).map(|e| e.iter().collect()).ok_or_else(|| EditError::Unported("audio without a track list".into()))
}

fn clips_of(track: &Value) -> Result<Vec<Value>, EditError> {
    track.get("clips").and_then(Value::elements).map(|e| e.iter().collect()).ok_or_else(|| EditError::Unported("an audio track without a clip list".into()))
}

/// `{...state, tracks, assets}` with only the recordings the tracks still use.
fn keep_used(state: &Value, tracks: Vec<Value>) -> Value {
    let used: std::collections::HashSet<String> =
        tracks.iter().flat_map(|t| t.get("clips").and_then(Value::elements).map(|e| e.iter().collect::<Vec<_>>()).unwrap_or_default()).filter_map(|c| c.get("assetId").and_then(Value::as_str).map(str::to_string)).collect();
    let mut assets = Obj::new();
    for (key, asset) in state.get("assets").and_then(Value::as_obj).map(|a| a.iter().collect::<Vec<_>>()).unwrap_or_default() {
        if used.contains(&**key) {
            assets.set(key, asset.clone());
        }
    }
    let next = with(state, "tracks", Value::array(tracks));
    with(&next, "assets", Value::object(assets))
}

impl Editor {
    /// `audioState(report)` and whether the document has audio (`hasOwn(_styk, 'audio')`).
    fn audio_state(&self) -> Result<(Value, bool), EditError> {
        let metadata = self.report.get("_styk").and_then(Value::as_obj).ok_or_else(|| EditError::Unported("audio in a document without metadata".into()))?;
        let present = metadata.contains("audio");
        Ok((metadata.get("audio").filter(|a| !a.is_null()).cloned().unwrap_or_else(empty_audio), present))
    }

    /// `publish(editor, next, label)`.
    fn publish_audio(&mut self, next: Value, label: &str) -> Result<bool, EditError> {
        let (before, before_present) = self.audio_state()?;
        validate_state(&next)?;
        if self.drag.is_some() {
            self.commit_drag()?;
        }
        if let Some(metadata) = self.report.make_obj().and_then(|r| r.get_mut("_styk")).and_then(Value::make_obj) {
            metadata.set("audio", next.clone());
        }
        let selection = self.selection_state();
        self.record(Command { kind: CommandKind::Audio { before, after: next, before_present }, label: label.into(), before_selection: selection.clone(), after_selection: selection, before_revision: 0, after_revision: 0 })?;
        self.emit("edit", vec![("label", Value::str(label)), ("audio", Value::Bool(true))]);
        Ok(true)
    }

    /// `applyAudioCommand(editor, command, direction)`: the audio as it was, the
    /// selection as it is.
    pub(crate) fn apply_audio_command(&mut self, command: &Command, direction: Direction) -> Result<(), EditError> {
        let CommandKind::Audio { before, after, before_present } = &command.kind else { return Ok(()) };
        if let Some(metadata) = self.report.make_obj().and_then(|r| r.get_mut("_styk")).and_then(Value::make_obj) {
            if direction == Direction::Before && !before_present {
                metadata.remove("audio");
            } else {
                metadata.set("audio", if direction == Direction::Before { before.clone() } else { after.clone() });
            }
        }
        self.revision = if direction == Direction::Before { command.before_revision } else { command.after_revision };
        self.emit("history", vec![("label", Value::str(&command.label)), ("audio", Value::Bool(true))]);
        Ok(())
    }

    /// `setAudioTrack(id, patch)`: name, volume, mute or solo.
    pub(crate) fn set_audio_track(&mut self, id: &Value, patch: &Value) -> Result<bool, EditError> {
        let patch = patch.as_obj().ok_or_else(|| EditError::Unported("an audio track change that is not an object".into()))?;
        if patch.keys().any(|key| !["name", "gain", "mute", "solo"].contains(&&**key)) {
            return Err(EditError::Invalid("errors.editorAudio.unsupportedAudioTrackProperty".into()));
        }
        let (state, _) = self.audio_state()?;
        let mut tracks = tracks_of(&state)?;
        let index = tracks.iter().position(|track| strict_eq(track.get("id"), Some(id))).ok_or_else(|| EditError::Invalid("errors.editorAudio.selectAnAudioTrack".into()))?;
        if patch.iter().all(|(key, value)| strict_eq(tracks[index].get(key), Some(value))) {
            return Ok(false);
        }
        let mut track = tracks[index].as_obj().cloned().unwrap_or_default();
        for (key, value) in patch.iter() {
            track.set(key, value.clone());
        }
        tracks[index] = Value::object(track);
        self.publish_audio(with(&state, "tracks", Value::array(tracks)), "Edit audio track")
    }

    /// `deleteAudioTrack(id)`: the track, and the recordings only it used.
    pub(crate) fn delete_audio_track(&mut self, id: &Value) -> Result<bool, EditError> {
        let (state, _) = self.audio_state()?;
        let tracks = tracks_of(&state)?;
        if !tracks.iter().any(|track| strict_eq(track.get("id"), Some(id))) {
            return Ok(false);
        }
        let kept = tracks.into_iter().filter(|track| !strict_eq(track.get("id"), Some(id))).collect();
        self.publish_audio(keep_used(&state, kept), "Delete audio track")
    }

    /// `clipOwner(editor, id)`: the track holding the clip, and the clip.
    fn clip_owner(&self, state: &Value, id: &Value) -> Result<(Value, Value), EditError> {
        for track in tracks_of(state)? {
            if let Some(clip) = clips_of(&track)?.into_iter().find(|clip| strict_eq(clip.get("id"), Some(id))) {
                return Ok((track, clip));
            }
        }
        Err(EditError::Invalid("errors.editorAudio.selectAnAudioClip".into()))
    }

    /// `setAudioClip(id, patch, targetTrackId)`: timing, volume and fades, and the track
    /// it is on. The clip changed goes last on its track.
    pub(crate) fn set_audio_clip(&mut self, id: &Value, patch: &Value, target: &Value) -> Result<bool, EditError> {
        let patch = patch.as_obj().ok_or_else(|| EditError::Unported("an audio clip change that is not an object".into()))?;
        if patch.keys().any(|key| !["start", "offset", "duration", "gain", "fadeIn", "fadeOut"].contains(&&**key)) {
            return Err(EditError::Invalid("errors.editorAudio.unsupportedAudioClipProperty".into()));
        }
        if patch.values().iter().any(|value| finite(Some(value)).is_none()) {
            return Err(EditError::Invalid("errors.editorAudio.audioClipPropertiesMustBeFiniteNumbers".into()));
        }
        let (state, _) = self.audio_state()?;
        let (track, clip) = self.clip_owner(&state, id)?;
        let owner = track.get("id").cloned();
        let target = if target.is_null() { owner.clone() } else { Some(target.clone()) };
        let tracks = tracks_of(&state)?;
        if !tracks.iter().any(|row| strict_eq(row.get("id"), target.as_ref())) {
            return Err(EditError::Invalid("errors.editorAudio.selectADestinationAudioTrack".into()));
        }
        let mut next = clip.as_obj().cloned().unwrap_or_default();
        for (key, value) in patch.iter() {
            next.set(key, value.clone());
        }
        // `Math.min(fade, duration)`.
        let field = |next: &Obj, key: &str| match next.get(key) {
            Some(Value::Num(n)) => Ok(*n),
            None => Ok(f64::NAN),
            Some(_) => Err(EditError::Unported(format!("an audio clip whose {key} is not a number"))),
        };
        let duration = field(&next, "duration")?;
        for key in ["fadeIn", "fadeOut"] {
            let fade = field(&next, key)?;
            next.set(key, Value::Num(if fade.is_nan() || duration.is_nan() { f64::NAN } else { fade.min(duration) }));
        }
        let same_track = strict_eq(target.as_ref(), owner.as_ref());
        if same_track && next.iter().all(|(key, value)| strict_eq(Some(value), clip.get(key))) {
            return Ok(false);
        }
        let next = Value::object(next);
        let without = |row: &Value| -> Result<Vec<Value>, EditError> { Ok(clips_of(row)?.into_iter().filter(|c| !strict_eq(c.get("id"), Some(id))).collect()) };
        let mut rows = Vec::with_capacity(tracks.len());
        for row in tracks {
            rows.push(if strict_eq(row.get("id"), target.as_ref()) {
                let mut clips = without(&row)?;
                clips.push(next.clone());
                with(&row, "clips", Value::array(clips))
            } else if strict_eq(row.get("id"), owner.as_ref()) {
                with(&row, "clips", Value::array(without(&row)?))
            } else {
                row
            });
        }
        self.publish_audio(with(&state, "tracks", Value::array(rows)), "Edit audio clip")
    }

    /// `deleteAudioClip(id)`: the clip, and its recording when nothing else uses it.
    pub(crate) fn delete_audio_clip(&mut self, id: &Value) -> Result<bool, EditError> {
        let (state, _) = self.audio_state()?;
        let (track, _) = self.clip_owner(&state, id)?;
        let owner = track.get("id").cloned();
        let mut rows = Vec::new();
        for row in tracks_of(&state)? {
            rows.push(if strict_eq(row.get("id"), owner.as_ref()) {
                let clips = clips_of(&row)?.into_iter().filter(|c| !strict_eq(c.get("id"), Some(id))).collect();
                with(&row, "clips", Value::array(clips))
            } else {
                row
            });
        }
        self.publish_audio(keep_used(&state, rows), "Delete audio clip")
    }
}
