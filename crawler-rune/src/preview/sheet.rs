//! Read-only spreadsheets. One sheet is loaded at a time into a flat cell arena: every cell's
//! text sits in one `String`, so a sheet costs four bytes per cell plus its text rather than an
//! allocation per cell. `.xlsx`, `.xlsm` and `.xlsb` stream cell by cell and stop at the limits;
//! `.xls` and `.ods` have no streaming reader, so calamine reads their sheet whole first.

use super::content::Budget;
use calamine::{Data, DataRef, ExcelDateTime, Reader, Sheets, open_workbook_auto};
use std::{fmt::Write, path::Path};

/// Bytes of one cell's text kept for display.
const CELL_BYTES: usize = 4096;
/// Rows measured for the column widths.
const MEASURED_ROWS: usize = 200;

#[derive(Clone, Copy)]
pub struct Limits {
    rows: usize,
    columns: usize,
    cells: usize,
}
impl Limits {
    pub fn of(budget: Budget) -> Self {
        match budget {
            Budget::Preview => Self {
                rows: 200,
                columns: 40,
                cells: 8_000,
            },
            Budget::Reader => Self {
                rows: 200_000,
                columns: 512,
                cells: 4_000_000,
            },
        }
    }
}

pub struct Sheet {
    pub columns: usize,
    /// Each column's width in points, from the longest of its first rows.
    pub widths: Vec<f32>,
    text: String,
    /// Where each cell's text ends in `text`; a cell starts where the one before it ends.
    ends: Vec<u32>,
    /// Index into `ends` of each row's first cell, plus one past the last row.
    rows: Vec<u32>,
}
impl Sheet {
    pub fn rows(&self) -> usize {
        self.rows.len() - 1
    }
    /// The cell's text, empty past the row's last filled cell.
    pub fn cell(&self, row: usize, column: usize) -> &str {
        let at = self.rows[row] as usize + column;
        if at >= self.rows[row + 1] as usize {
            return "";
        }
        let start = if at == 0 {
            0
        } else {
            self.ends[at - 1] as usize
        };
        &self.text[start..self.ends[at] as usize]
    }
}

/// The sheet names and the one loaded sheet. Delimited text has no names.
pub struct Workbook {
    pub names: Vec<String>,
    pub active: usize,
    pub sheet: Sheet,
}

struct Builder {
    limits: Limits,
    text: String,
    ends: Vec<u32>,
    rows: Vec<u32>,
    columns: usize,
    truncated: bool,
}
impl Builder {
    fn new(limits: Limits) -> Self {
        Self {
            limits,
            text: String::new(),
            ends: Vec::new(),
            rows: vec![0],
            columns: 0,
            truncated: false,
        }
    }
    fn row_cells(&self) -> usize {
        self.ends.len() - *self.rows.last().expect("rows start at zero") as usize
    }
    /// Appends a cell to the open row. Returns false once the sheet is full.
    fn cell(&mut self, value: &str) -> bool {
        if self.row_cells() >= self.limits.columns {
            self.truncated |= !value.is_empty();
            return true;
        }
        if self.ends.len() >= self.limits.cells
            || self.text.len() + value.len().min(CELL_BYTES) > u32::MAX as usize
        {
            self.truncated = true;
            return false;
        }
        let mut end = value.len().min(CELL_BYTES);
        while !value.is_char_boundary(end) {
            end -= 1;
        }
        self.text.push_str(&value[..end]);
        self.ends.push(self.text.len() as u32);
        true
    }
    /// Pads the open row with empty cells up to `column`.
    fn pad(&mut self, column: usize) -> bool {
        while self.row_cells() < column.min(self.limits.columns) {
            if !self.cell("") {
                return false;
            }
        }
        true
    }
    /// Closes the open row. Returns false once the sheet is full.
    fn end_row(&mut self) -> bool {
        let start = *self.rows.last().expect("rows start at zero") as usize;
        // Trailing empty cells would widen the sheet without showing anything.
        while self.ends.len() > start
            && self.ends[self.ends.len() - 1] == self.ends.len().checked_sub(2).map_or(0, |i| self.ends[i])
        {
            self.ends.pop();
        }
        self.columns = self.columns.max(self.ends.len() - start);
        self.rows.push(self.ends.len() as u32);
        if self.rows.len() > self.limits.rows {
            self.truncated = true;
            return false;
        }
        true
    }
    fn finish(mut self) -> (Sheet, bool) {
        if self.row_cells() > 0 {
            self.end_row();
        }
        // Empty rows after the last filled one are not part of the sheet.
        while self.rows.len() > 1 && self.rows[self.rows.len() - 2] as usize == self.ends.len() {
            self.rows.pop();
        }
        let mut sheet = Sheet {
            columns: self.columns,
            widths: Vec::new(),
            text: self.text,
            ends: self.ends,
            rows: self.rows,
        };
        sheet.widths = (0..sheet.columns)
            .map(|column| {
                let characters = (0..sheet.rows().min(MEASURED_ROWS))
                    .map(|row| sheet.cell(row, column).chars().take(48).count())
                    .max()
                    .unwrap_or(0);
                (characters as f32 * 7.2 + 24.0).clamp(64.0, 360.0)
            })
            .collect();
        (sheet, self.truncated)
    }
}

/// Loads sheet `active` of a workbook. Returns whether the limits cut it short.
pub fn workbook(
    path: &Path,
    active: usize,
    limits: Limits,
    cancelled: &impl Fn() -> bool,
) -> Result<(Workbook, bool), String> {
    let mut sheets = open_workbook_auto(path).map_err(|e| e.to_string())?;
    let names = sheets.sheet_names();
    let active = active.min(names.len().saturating_sub(1));
    let name = names
        .get(active)
        .ok_or("This workbook has no sheets.")?
        .clone();
    let mut builder = Builder::new(limits);
    match &mut sheets {
        Sheets::Xlsx(book) => {
            let mut cells = book.worksheet_cells_reader(&name).map_err(|e| e.to_string())?;
            stream(
                || cells.next_cell().map_err(|e| e.to_string()),
                &mut builder,
                cancelled,
            )?;
        }
        Sheets::Xlsb(book) => {
            let mut cells = book.worksheet_cells_reader(&name).map_err(|e| e.to_string())?;
            stream(
                || cells.next_cell().map_err(|e| e.to_string()),
                &mut builder,
                cancelled,
            )?;
        }
        Sheets::Xls(_) | Sheets::Ods(_) => {
            let range = sheets.worksheet_range(&name).map_err(|e| e.to_string())?;
            let (first_row, first_column) = range.start().unwrap_or((0, 0));
            let mut value = String::new();
            'rows: {
                for _ in 0..first_row {
                    if !builder.end_row() {
                        break 'rows;
                    }
                }
                for (index, row) in range.rows().enumerate() {
                    if index % 1024 == 0 && cancelled() {
                        return Err("Cancelled".into());
                    }
                    if !builder.pad(first_column as usize) {
                        break 'rows;
                    }
                    for cell in row {
                        value.clear();
                        write_data(&mut value, cell);
                        if !builder.cell(&value) {
                            break 'rows;
                        }
                    }
                    if !builder.end_row() {
                        break 'rows;
                    }
                }
            }
        }
    }
    let (sheet, truncated) = builder.finish();
    Ok((
        Workbook {
            names,
            active,
            sheet,
        },
        truncated,
    ))
}

/// Cells arrive in row order with gaps for empty ones.
fn stream<'a>(
    mut next: impl FnMut() -> Result<Option<calamine::Cell<DataRef<'a>>>, String>,
    builder: &mut Builder,
    cancelled: &impl Fn() -> bool,
) -> Result<(), String> {
    let mut row = 0;
    let mut value = String::new();
    let mut read = 0usize;
    while let Some(cell) = next()? {
        read += 1;
        if read % 4096 == 0 && cancelled() {
            return Err("Cancelled".into());
        }
        if matches!(cell.get_value(), DataRef::Empty) {
            continue;
        }
        let (at_row, at_column) = cell.get_position();
        while row < at_row {
            if !builder.end_row() {
                return Ok(());
            }
            row += 1;
        }
        // A cell before the open row's end is out of order; a viewer skips it.
        if at_row < row || (at_column as usize) < builder.row_cells() {
            continue;
        }
        value.clear();
        match cell.get_value() {
            DataRef::SharedString(text) => value.push_str(text),
            other => write_data(&mut value, &Data::from(other.clone())),
        }
        if !builder.pad(at_column as usize) || !builder.cell(&value) {
            return Ok(());
        }
    }
    Ok(())
}

fn write_data(out: &mut String, value: &Data) {
    let _ = match value {
        Data::Int(v) => write!(out, "{v}"),
        Data::Float(v) => {
            // Fifteen significant digits, as spreadsheets show: 0.1 + 0.2 reads 0.3.
            let rounded: f64 = format!("{v:.14e}").parse().unwrap_or(*v);
            write!(out, "{rounded}")
        }
        Data::String(v) | Data::DateTimeIso(v) | Data::DurationIso(v) => {
            out.push_str(v);
            Ok(())
        }
        Data::Bool(v) => {
            out.push_str(if *v { "TRUE" } else { "FALSE" });
            Ok(())
        }
        Data::DateTime(v) => write_date(out, v),
        Data::Error(v) => write!(out, "{v}"),
        Data::Empty => Ok(()),
    };
}

fn write_date(out: &mut String, value: &ExcelDateTime) -> std::fmt::Result {
    let serial = value.as_f64();
    if value.is_duration() {
        let seconds = (serial * 86_400.0).round() as i64;
        return write!(
            out,
            "{}:{:02}:{:02}",
            seconds / 3600,
            seconds / 60 % 60,
            seconds % 60
        );
    }
    let (year, month, day, hour, minute, second, _) = value.to_ymd_hms_milli();
    if serial < 1.0 {
        write!(out, "{hour:02}:{minute:02}:{second:02}")
    } else if serial.fract() == 0.0 {
        write!(out, "{year:04}-{month:02}-{day:02}")
    } else {
        write!(
            out,
            "{year:04}-{month:02}-{day:02} {hour:02}:{minute:02}:{second:02}"
        )
    }
}

/// Comma, semicolon or tab separated text with RFC 4180 quoting.
pub fn delimited(source: &str, tabs: bool, limits: Limits) -> (Workbook, bool) {
    let delimiter = if tabs { b'\t' } else { sniff(source) };
    let mut builder = Builder::new(limits);
    let mut field = String::new();
    let mut quoted = false;
    let mut bytes = source.bytes().enumerate().peekable();
    let mut start = 0;
    // Field text between structural bytes is copied in whole runs, so multi-byte
    // characters never split: every structural byte is ASCII.
    let mut full = false;
    while let Some((at, byte)) = bytes.next() {
        if quoted {
            if byte == b'"' {
                field.push_str(&source[start..at]);
                if bytes.peek().is_some_and(|(_, next)| *next == b'"') {
                    field.push('"');
                    bytes.next();
                } else {
                    quoted = false;
                }
                start = bytes.peek().map_or(source.len(), |(next, _)| *next);
            }
            continue;
        }
        match byte {
            b'"' if at == start && field.is_empty() => {
                quoted = true;
                start = at + 1;
            }
            b'\n' | b'\r' => {
                field.push_str(&source[start..at]);
                if byte == b'\r' && bytes.peek().is_some_and(|(_, next)| *next == b'\n') {
                    bytes.next();
                }
                start = bytes.peek().map_or(source.len(), |(next, _)| *next);
                if !builder.cell(&field) || !builder.end_row() {
                    full = true;
                    break;
                }
                field.clear();
            }
            _ if byte == delimiter => {
                field.push_str(&source[start..at]);
                start = at + 1;
                if !builder.cell(&field) {
                    full = true;
                    break;
                }
                field.clear();
            }
            _ => {}
        }
    }
    if !full {
        field.push_str(&source[start..]);
        if !field.is_empty() {
            builder.cell(&field);
        }
    }
    let (sheet, truncated) = builder.finish();
    (
        Workbook {
            names: Vec::new(),
            active: 0,
            sheet,
        },
        truncated,
    )
}

/// The separator that appears most in the first line, outside quotes.
fn sniff(source: &str) -> u8 {
    let mut counts = [0usize; 3];
    let mut quoted = false;
    for byte in source.bytes() {
        match byte {
            b'"' => quoted = !quoted,
            b'\n' if !quoted => break,
            b',' if !quoted => counts[0] += 1,
            b';' if !quoted => counts[1] += 1,
            b'\t' if !quoted => counts[2] += 1,
            _ => {}
        }
    }
    let best = (0..3).max_by_key(|i| (counts[*i], 2 - *i)).unwrap_or(0);
    [b',', b';', b'\t'][best]
}
