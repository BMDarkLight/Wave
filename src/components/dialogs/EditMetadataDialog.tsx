/*
 * Wave
 * Copyright (C) 2025 BMDarkLight
 *
 * Licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).
 * See the LICENSE file in the project root for the full license text
 * and additional terms (attribution and fork-marking requirements).
 * https://github.com/BMDarkLight/Wave
 */

import { useEffect, useMemo, useState } from "react";
import { BiImageAlt, BiX } from "react-icons/bi";
import {
  checkMetadataWriteAccess,
  readCoverPreview,
  resolveCoverSrc,
  selectCoverImage,
} from "../../utils/player";
import type { TagEdit, Track } from "../../utils/player";

/** Fields the dialog edits, in the order they appear. */
const FIELDS = [
  "title",
  "artist",
  "album",
  "album_artist",
  "genre",
  "year",
  "track_number",
  "disc_number",
] as const;

type Field = (typeof FIELDS)[number];

const LABELS: Record<Field, string> = {
  title: "Title",
  artist: "Artist",
  album: "Album",
  album_artist: "Album artist",
  genre: "Genre",
  year: "Year",
  track_number: "Track number",
  disc_number: "Disc number",
};

const NUMERIC: Field[] = ["year", "track_number", "disc_number"];

/** Fields that take the full dialog width instead of sharing a row. */
const WIDE: Field[] = ["title", "artist", "album"];

/** Fields that Wave always needs a value for. */
const REQUIRED: Field[] = ["title", "artist", "album"];

const valueOf = (track: Track, field: Field): string => {
  const value = track[field];
  if (value == null) return "";
  return String(value);
};

/**
 * One value when every track agrees on it, otherwise null. A null field starts
 * out blank and is only sent if the user types something, which is what keeps a
 * batch edit from flattening titles that differ.
 */
const sharedValues = (tracks: Track[]): Record<Field, string | null> => {
  const shared = {} as Record<Field, string | null>;
  for (const field of FIELDS) {
    const first = tracks[0] ? valueOf(tracks[0], field) : "";
    shared[field] = tracks.every((track) => valueOf(track, field) === first)
      ? first
      : null;
  }
  return shared;
};

type CoverChoice =
  { kind: "keep" } | { kind: "remove" } | { kind: "replace"; path: string };

export default function EditMetadataDialog({
  tracks,
  onClose,
  onSave,
  onGrantWriteAccess,
}: {
  tracks: Track[];
  onClose: () => void;
  onSave: (edit: TagEdit) => Promise<void>;
  /** Re-runs the folder picker so a read-only grant can be upgraded. */
  onGrantWriteAccess?: () => void;
}) {
  const shared = useMemo(() => sharedValues(tracks), [tracks]);
  const [values, setValues] = useState<Record<Field, string>>(() => {
    const initial = {} as Record<Field, string>;
    for (const field of FIELDS) initial[field] = shared[field] ?? "";
    return initial;
  });
  const [cover, setCover] = useState<CoverChoice>({ kind: "keep" });
  const [coverSrc, setCoverSrc] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);
  const [writable, setWritable] = useState(true);

  const single = tracks.length === 1;
  const subject = single
    ? tracks[0].title || tracks[0].name
    : `${tracks.length} tracks`;

  useEffect(() => {
    let cancelled = false;
    void (async () => {
      if (cover.kind === "remove") {
        setCoverSrc(null);
        return;
      }
      const src =
        cover.kind === "replace"
          ? await readCoverPreview(cover.path)
          : await resolveCoverSrc(tracks[0]?.cover_art_data_url);
      if (!cancelled) setCoverSrc(src);
    })();
    return () => {
      cancelled = true;
    };
  }, [cover, tracks]);

  useEffect(() => {
    let cancelled = false;
    void (async () => {
      const ok = await checkMetadataWriteAccess(tracks.map((t) => t.path));
      if (!cancelled) setWritable(ok);
    })();
    return () => {
      cancelled = true;
    };
  }, [tracks]);

  const setField = (field: Field, value: string) => {
    setValues((current) => ({ ...current, [field]: value }));
  };

  const pickCover = async () => {
    try {
      const path = await selectCoverImage();
      if (path) setCover({ kind: "replace", path });
    } catch {
      setError("Could not open the image picker.");
    }
  };

  const submit = async () => {
    const edit: TagEdit = {};
    for (const field of FIELDS) {
      const before = shared[field];
      // A field nobody touched stays out of the edit entirely. For mixed
      // values `before` is null, so an untouched blank box sends nothing.
      if (values[field] === (before ?? "")) continue;
      if (REQUIRED.includes(field) && values[field].trim() === "") {
        setError(`${LABELS[field]} cannot be empty.`);
        return;
      }
      edit[field] = values[field];
    }
    if (cover.kind === "replace") {
      edit.cover = { action: "replace", path: cover.path };
    } else if (cover.kind === "remove") {
      edit.cover = { action: "remove" };
    }

    if (Object.keys(edit).length === 0) {
      onClose();
      return;
    }

    setSaving(true);
    setError(null);
    try {
      await onSave(edit);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setSaving(false);
    }
  };

  return (
    <div className="modal-backdrop" onClick={onClose}>
      <div
        className="modal-dialog metadata-dialog"
        onClick={(event) => event.stopPropagation()}
        onKeyDown={(event) => {
          if (event.key === "Escape") onClose();
        }}
      >
        <div className="modal-header">
          <h2>Edit metadata</h2>
          <button
            className="modal-close-btn"
            onClick={onClose}
            type="button"
            title="Close"
          >
            <BiX />
          </button>
        </div>

        <p className="modal-hint">
          {single ? subject : `Editing ${subject} at once.`}
          {!single &&
            " Fields left blank where the tracks disagree are not changed."}
        </p>

        {!writable && (
          <div className="modal-warning" role="status">
            <span>
              This folder was added without write access, so saving will
              probably fail. Adding it again grants Wave permission to write.
            </span>
            {onGrantWriteAccess && (
              <button
                type="button"
                className="btn-ghost btn-sm"
                onClick={onGrantWriteAccess}
              >
                Add folder again
              </button>
            )}
          </div>
        )}

        <form
          onSubmit={(event) => {
            event.preventDefault();
            void submit();
          }}
        >
          <div className="metadata-cover">
            {coverSrc ? (
              <img src={coverSrc} alt="Cover art" />
            ) : (
              <div className="metadata-cover-empty">
                <BiImageAlt />
              </div>
            )}
            <div className="metadata-cover-actions">
              <button
                type="button"
                className="btn-ghost btn-sm"
                onClick={() => void pickCover()}
              >
                Choose image
              </button>
              <button
                type="button"
                className="btn-ghost btn-sm"
                onClick={() => setCover({ kind: "remove" })}
              >
                Remove
              </button>
              {cover.kind !== "keep" && (
                <button
                  type="button"
                  className="btn-ghost btn-sm"
                  onClick={() => setCover({ kind: "keep" })}
                >
                  Undo
                </button>
              )}
            </div>
          </div>

          <div className="metadata-fields">
            {FIELDS.map((field) => (
              <div
                className={`metadata-field${WIDE.includes(field) ? " metadata-field-wide" : ""}`}
                key={field}
              >
                <label className="modal-label" htmlFor={`metadata-${field}`}>
                  {LABELS[field]}
                </label>
                <input
                  id={`metadata-${field}`}
                  className="modal-input"
                  type={NUMERIC.includes(field) ? "number" : "text"}
                  min={NUMERIC.includes(field) ? 1 : undefined}
                  max={NUMERIC.includes(field) ? 9999 : undefined}
                  value={values[field]}
                  onChange={(event) => setField(field, event.target.value)}
                  placeholder={shared[field] === null ? "Multiple values" : ""}
                  autoComplete="off"
                />
              </div>
            ))}
          </div>

          {error && <p className="modal-error">{error}</p>}

          <div className="modal-actions">
            <button className="btn-ghost" onClick={onClose} type="button">
              Cancel
            </button>
            <button className="btn-primary" type="submit" disabled={saving}>
              {saving ? "Saving…" : "Save"}
            </button>
          </div>
        </form>
      </div>
    </div>
  );
}
