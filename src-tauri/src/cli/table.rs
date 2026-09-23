// Wave
// Copyright (C) 2025 BMDarkLight
//
// Licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).
// See the LICENSE file in the project root for the full license text
// and additional terms (attribution and fork-marking requirements).
// https://github.com/BMDarkLight/Wave

//! Column table that fits whatever terminal it is printed into.
//!
//! Columns declare a minimum width and a growth weight. Everything gets its
//! natural width when there is room; otherwise the surplus is handed out by
//! weight, so a duration column never shrinks to make room for a long title.

pub use crate::cli::ui::Align;
use crate::cli::ui::{display_width, pad, truncate, Ui};

/// Below this many columns a row cannot be laid out side by side, so the
/// table stacks each record onto its own lines instead.
pub const STACK_BELOW: usize = 60;

const GUTTER: usize = 2;
const INDENT: usize = 2;

pub struct Column {
    pub header: &'static str,
    pub min: usize,
    pub weight: u16,
    pub align: Align,
}

pub struct Table {
    columns: Vec<Column>,
    rows: Vec<Vec<String>>,
}

impl Table {
    pub fn new(columns: Vec<Column>) -> Table {
        Table {
            columns,
            rows: Vec::new(),
        }
    }

    pub fn push(&mut self, row: Vec<String>) {
        debug_assert_eq!(row.len(), self.columns.len(), "row does not match columns");
        self.rows.push(row);
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    pub fn render(&self, ui: &Ui) -> String {
        if self.rows.is_empty() {
            return String::new();
        }
        if ui.width < STACK_BELOW {
            return self.render_stacked(ui);
        }
        self.render_columns(ui)
    }

    /// Widest of each column's header and cells, never below its minimum.
    fn natural_widths(&self) -> Vec<usize> {
        self.columns
            .iter()
            .enumerate()
            .map(|(i, col)| {
                let widest = self
                    .rows
                    .iter()
                    .map(|row| display_width(&row[i]))
                    .max()
                    .unwrap_or(0);
                widest.max(display_width(col.header)).max(col.min)
            })
            .collect()
    }

    fn solve_widths(&self, available: usize) -> Vec<usize> {
        let natural = self.natural_widths();
        let gutters = GUTTER * self.columns.len().saturating_sub(1);
        let budget = available.saturating_sub(INDENT + gutters);
        if natural.iter().sum::<usize>() <= budget {
            return natural;
        }

        // Start everyone at their minimum, then share what is left by weight.
        let mut widths: Vec<usize> = self.columns.iter().map(|c| c.min).collect();
        let min_total: usize = widths.iter().sum();
        if min_total >= budget {
            return widths;
        }
        let spare = budget - min_total;
        let weight_total: usize = self.columns.iter().map(|c| c.weight as usize).sum();
        if weight_total == 0 {
            return widths;
        }

        // First pass: proportional shares, capped at what a column can use.
        let mut surplus = spare;
        for (i, col) in self.columns.iter().enumerate() {
            let share = spare * col.weight as usize / weight_total;
            let give = share.min(natural[i].saturating_sub(widths[i])).min(surplus);
            widths[i] += give;
            surplus -= give;
        }

        // Integer division and capped columns leave some over. Hand it out a
        // column at a time, heaviest first, so no space is wasted.
        let mut order: Vec<usize> = (0..self.columns.len())
            .filter(|&i| self.columns[i].weight > 0)
            .collect();
        order.sort_by_key(|&i| std::cmp::Reverse(self.columns[i].weight));
        while surplus > 0 {
            let mut progressed = false;
            for &i in &order {
                if surplus > 0 && widths[i] < natural[i] {
                    widths[i] += 1;
                    surplus -= 1;
                    progressed = true;
                }
            }
            if !progressed {
                break;
            }
        }

        widths
    }

    fn render_columns(&self, ui: &Ui) -> String {
        let widths = self.solve_widths(ui.width);
        let indent = " ".repeat(INDENT);
        let gutter = " ".repeat(GUTTER);
        let mut out = String::new();

        let header: Vec<String> = self
            .columns
            .iter()
            .zip(&widths)
            .map(|(col, w)| pad(col.header, *w, col.align))
            .collect();
        out.push_str(&indent);
        out.push_str(&ui.dim(header.join(&gutter).trim_end()));
        out.push('\n');

        let rule: Vec<String> = widths.iter().map(|w| ui.glyphs.rule.repeat(*w)).collect();
        out.push_str(&indent);
        out.push_str(&ui.dim(&rule.join(&gutter)));
        out.push('\n');

        for row in &self.rows {
            let cells: Vec<String> = row
                .iter()
                .zip(self.columns.iter().zip(&widths))
                .map(|(cell, (col, w))| pad(&truncate(cell, *w, ui.glyphs.ellipsis), *w, col.align))
                .collect();
            out.push_str(&indent);
            out.push_str(cells.join(&gutter).trim_end());
            out.push('\n');
        }

        out
    }

    /// One record per block, labelled, for terminals too narrow for columns.
    fn render_stacked(&self, ui: &Ui) -> String {
        let label_width = self
            .columns
            .iter()
            .map(|c| display_width(c.header))
            .max()
            .unwrap_or(0);
        let value_width = ui.width.saturating_sub(INDENT + label_width + GUTTER);
        let indent = " ".repeat(INDENT);
        let gutter = " ".repeat(GUTTER);
        let mut out = String::new();

        for (n, row) in self.rows.iter().enumerate() {
            if n > 0 {
                out.push('\n');
            }
            for (cell, col) in row.iter().zip(&self.columns) {
                out.push_str(&indent);
                out.push_str(&ui.dim(&pad(col.header, label_width, Align::Left)));
                out.push_str(&gutter);
                out.push_str(&truncate(cell, value_width, ui.glyphs.ellipsis));
                out.push('\n');
            }
        }

        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::ui::{display_width, Ui};

    fn columns() -> Vec<Column> {
        vec![
            Column {
                header: "ID",
                min: 8,
                weight: 0,
                align: Align::Left,
            },
            Column {
                header: "DUR",
                min: 5,
                weight: 0,
                align: Align::Right,
            },
            Column {
                header: "ARTIST",
                min: 8,
                weight: 1,
                align: Align::Left,
            },
            Column {
                header: "TITLE",
                min: 8,
                weight: 2,
                align: Align::Left,
            },
        ]
    }

    fn render_one(width: usize, artist: &str, title: &str) -> String {
        let mut t = Table::new(columns());
        t.push(vec![
            "3f2a91c4".into(),
            "04:20".into(),
            artist.into(),
            title.into(),
        ]);
        let ui = Ui {
            width,
            ..Ui::plain()
        };
        t.render(&ui)
    }

    fn sample(width: usize) -> String {
        render_one(width, "Talking Heads", "Once in a Lifetime")
    }

    #[test]
    fn every_line_fits_the_terminal() {
        for width in [60, 80, 100, 200] {
            for line in sample(width).lines() {
                assert!(
                    display_width(line) <= width,
                    "width {width} produced a {} column line: {line:?}",
                    display_width(line)
                );
            }
        }
    }

    #[test]
    fn fixed_columns_keep_their_width_when_space_is_tight() {
        // Natural width here is far past 62 columns, so ARTIST and TITLE must
        // give ground while ID and DUR, which have no weight, keep theirs.
        let out = render_one(
            62,
            "Talking Heads and the Tom Tom Club",
            "Once in a Lifetime (Live at the Pantages Theatre)",
        );
        let header = out.lines().next().unwrap();
        assert!(header.contains("ID"));
        assert!(header.contains("DUR"));
        assert!(out.contains("3f2a91c4"), "id cell was cut: {out}");
        assert!(out.contains("04:20"), "duration cell was cut: {out}");
        assert!(
            out.contains('\u{2026}'),
            "expected the long cells to truncate"
        );
        for line in out.lines() {
            assert!(display_width(line) <= 62, "line too wide: {line:?}");
        }
    }

    #[test]
    fn surplus_goes_to_the_heaviest_column() {
        let out = sample(200);
        // At a generous width both flexible columns print in full.
        assert!(out.contains("Talking Heads"));
        assert!(out.contains("Once in a Lifetime"));
        assert!(!out.contains('\u{2026}'));
    }

    #[test]
    fn narrow_terminals_stack_instead_of_mangling_rows() {
        let ui = Ui::plain();
        let out = sample(40);
        for line in out.lines() {
            assert!(display_width(line) <= 40, "stacked line too wide: {line:?}");
        }
        // No header rule, and one labelled line per column for a single row.
        assert!(!out.contains(&ui.glyphs.rule.repeat(8)));
        assert_eq!(out.lines().count(), 4);
        for label in ["ID", "DUR", "ARTIST", "TITLE"] {
            assert!(
                out.lines().any(|l| l.trim_start().starts_with(label)),
                "no line labelled {label}: {out}"
            );
        }
    }

    #[test]
    fn an_empty_table_renders_nothing() {
        let t = Table::new(columns());
        assert!(t.is_empty());
        assert_eq!(t.render(&Ui::plain()), "");
    }

    #[test]
    fn content_wider_than_its_column_is_truncated_not_wrapped() {
        let out = render_one(80, &"A".repeat(200), &"B".repeat(200));
        assert_eq!(out.lines().count(), 3, "expected header, rule, one row");
        for line in out.lines() {
            assert!(display_width(line) <= 80);
        }
    }
}
