/*
 * Copyright (c) 2026 spamska and contributors
 * All rights reserved.
 *
 * Redistribution and use in source and binary forms, with or without
 * modification, are permitted provided that the following conditions
 * are met:
 * 1. Redistributions of source code must retain the above copyright
 *    notice, this list of conditions and the following disclaimer.
 * 2. Redistributions in binary form must reproduce the above copyright
 *    notice, this list of conditions and the following disclaimer in the
 *    documentation and/or other materials provided with the distribution.
 *
 * THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS
 * "AS IS" AND ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT
 * LIMITED TO, THE IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS
 * FOR A PARTICULAR PURPOSE ARE DISCLAIMED. IN NO EVENT SHALL THE
 * COPYRIGHT HOLDER OR CONTRIBUTORS BE LIABLE FOR ANY DIRECT, INDIRECT,
 * INCIDENTAL, SPECIAL, EXEMPLARY, OR CONSEQUENTIAL DAMAGES (INCLUDING,
 * BUT NOT LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR SERVICES;
 * LOSS OF USE, DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER CAUSED
 * AND ON ANY THEORY OF LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY,
 * OR TORT (INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE
 * USE OF THIS SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.
 *
 * NOTE: This file is licensed under the BSD 2-Clause license. 
 * This does not imply that all components or parts of the Ifsy 
 * operating system are distributed under the same terms. Please consult 
 * the individual components or the root license documentation for details.
 */

use core::fmt;
use super::font::{self, GLYPH_HEIGHT, GLYPH_WIDTH};

const GRID_COLS: usize = 106;
const GRID_ROWS: usize = 50;
const CELL_UNITS_W: usize = GLYPH_WIDTH + 1;
const CELL_UNITS_H: usize = GLYPH_HEIGHT + 1;
const MIN_CELL_H: usize = CELL_UNITS_H;
const MAX_CELL_W: usize = 96;
const MAX_CELL_H: usize = 128;

const PALETTE: [u32; 16] = [
    0x00000000,
    0x00AA0000,
    0x0000AA00,
    0x00AA5500,
    0x000000AA,
    0x00AA00AA,
    0x0000AAAA,
    0x00AAAAAA,
    0x00555555,
    0x00FF5555,
    0x0055FF55,
    0x00FFFF55,
    0x005555FF,
    0x00FF55FF,
    0x0055FFFF,
    0x00FFFFFF,
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AnsiState {
    Normal,
    Escaped,
    Csi,
}

pub struct TextConsole {
    base: *mut u8,
    pitch: usize,
    width: usize,
    height: usize,
    bytes_per_pixel: usize,
    cell_w: usize,
    cell_h: usize,
    x0: usize,
    y0: usize,
    xmap: [u8; MAX_CELL_W],
    ymap: [u8; MAX_CELL_H],
    cols: usize,
    rows: usize,
    cursor_col: usize,
    cursor_row: usize,

    ansi_state: AnsiState,
    ansi_buffer: [u8; 16],
    ansi_len: usize,

    fg_color: u32,
    bg_color: u32,

    cursor_drawn: bool,
    blink_on: bool,
    cursor_enabled: bool,
}

// SAFETY: access to the inner value is serialized by the lock (or the data is only moved once), so sharing across threads cannot race.
unsafe impl Send for TextConsole {}

/// # Safety
/// Caller must be at kernel privilege and have met this function's hardware and initialization prerequisites.
unsafe fn copy_up(dst: *mut u8, src: *const u8, bytes: usize) {
    #[cfg(target_arch = "x86_64")]
    if bytes % 8 == 0 && dst as usize % 8 == 0 && src as usize % 8 == 0 {
        // SAFETY: privileged register/instruction; this code runs only at kernel privilege and the architectural effect (translation, interrupt or FPU state) is what the surrounding code intends.
        unsafe {
            core::arch::asm!(
                "rep movsq",
                inout("rcx") bytes / 8 => _,
                inout("rdi") dst => _,
                inout("rsi") src => _,
                options(nostack, preserves_flags)
            );
        }
        return;
    }
    // SAFETY: the pointer stays inside the framebuffer region handed to attach_framebuffer and the console has exclusive access.
    unsafe { core::ptr::copy(src, dst, bytes) };
}

impl TextConsole {
    /// # Safety
    /// called at kernel privilege in the boot/driver order set up by kmain; the callee's own contract (hardware present, prerequisites initialized, interrupts masked where required) is met at this call site.
    pub unsafe fn new(base: *mut u8, width: usize, height: usize, pitch: usize, bytes_per_pixel: usize) -> Self {
        let mut t = Self {
            cell_w: CELL_UNITS_W,
            cell_h: CELL_UNITS_H,
            x0: 0,
            y0: 0,
            xmap: [0; MAX_CELL_W],
            ymap: [0; MAX_CELL_H],
            base,
            pitch,
            width,
            height,
            bytes_per_pixel,
            cols: 0,
            rows: 0,
            cursor_col: 0,
            cursor_row: 0,
            ansi_state: AnsiState::Normal,
            ansi_buffer: [0; 16],
            ansi_len: 0,
            fg_color: 0xFFFFFFFF,
            bg_color: 0x00000000,
            cursor_drawn: false,
            blink_on: true,
            cursor_enabled: false,
        };
        let (c, r) = t.default_grid();
        if t.layout(c, r).is_err() {
            t.cols = 0;
            t.rows = 0;
        }
        t
    }

    pub fn default_grid(&self) -> (usize, usize) {
        let big_w = CELL_UNITS_W * 2;
        let big_h = CELL_UNITS_H * 2;
        if self.width >= GRID_COLS * big_w && self.height >= GRID_ROWS * big_h {
            (self.width / big_w, self.height / big_h)
        } else {
            (GRID_COLS, GRID_ROWS)
        }
    }

    pub fn max_grid(&self) -> (usize, usize) {
        (self.width / CELL_UNITS_W, self.height / MIN_CELL_H)
    }

    fn layout(&mut self, cols: usize, rows: usize) -> Result<(), &'static str> {
        if cols == 0 || rows == 0 {
            return Err("size must be at least 1x1");
        }
        let by_h = self.height / rows;
        let by_w = (self.width / cols) * CELL_UNITS_H / CELL_UNITS_W;
        let ch = by_h.min(by_w).min(MAX_CELL_H);
        let cw = ch * CELL_UNITS_W / CELL_UNITS_H;
        if ch < MIN_CELL_H || cw < CELL_UNITS_W {
            return Err("too large for the screen");
        }
        if cw > MAX_CELL_W {
            return Err("too small for the screen");
        }
        self.cell_w = cw;
        self.cell_h = ch;
        self.cols = cols;
        self.rows = rows;
        self.x0 = (self.width - cols * cw) / 2;
        self.y0 = (self.height - rows * ch) / 2;
        for px in 0..cw {
            self.xmap[px] = (px * CELL_UNITS_W / cw) as u8;
        }
        for py in 0..ch {
            self.ymap[py] = (py * CELL_UNITS_H / ch) as u8;
        }
        Ok(())
    }

    pub fn set_grid(&mut self, cols: usize, rows: usize) -> Result<(), &'static str> {
        self.layout(cols, rows)?;
        self.clear();
        Ok(())
    }

    pub fn geometry(&self) -> (usize, usize, usize, usize) {
        (self.width, self.height, self.cell_w, self.cell_h)
    }

    fn fill_rect(&mut self, x: usize, y: usize, w: usize, h: usize, color: u32) {
        let x_end = (x + w).min(self.width);
        let y_end = (y + h).min(self.height);
        for py in y..y_end {
            // SAFETY: the address is a device register or ring/DMA slot mapped by this driver's init and aligned for the access width; volatile stops the compiler eliding or tearing the access.
            let mut p = unsafe { self.base.add(py * self.pitch + x * self.bytes_per_pixel) }.cast::<u32>();
            for _ in x..x_end {
                // SAFETY: the address is a device register or ring/DMA slot mapped by this driver's init and aligned for the access width; volatile stops the compiler eliding or tearing the access.
                unsafe {
                    p.write_volatile(color);
                    p = p.add(1);
                }
            }
        }
    }

    fn draw_glyph(&mut self, c: char, col: usize, row: usize) {
        let bitmap = font::glyph(c);
        let origin_x = self.x0 + col * self.cell_w;
        let origin_y = self.y0 + row * self.cell_h;
        if origin_x + self.cell_w > self.width || origin_y + self.cell_h > self.height {
            return;
        }
        let mut line = [0u32; MAX_CELL_W];
        for py in 0..self.cell_h {
            let bits = bitmap.get(self.ymap[py] as usize).copied();
            for px in 0..self.cell_w {
                let gx = self.xmap[px] as usize;
                let on = gx < GLYPH_WIDTH && bits.is_some_and(|b| b & (1 << (GLYPH_WIDTH - 1 - gx)) != 0);
                line[px] = if on { self.fg_color } else { self.bg_color };
            }
            // SAFETY: the address is a device register or ring/DMA slot mapped by this driver's init and aligned for the access width; volatile stops the compiler eliding or tearing the access.
            let dst = unsafe { self.base.add((origin_y + py) * self.pitch + origin_x * self.bytes_per_pixel) }.cast::<u32>();
            for (i, px) in line[..self.cell_w].iter().enumerate() {
                // SAFETY: the address is a device register or ring/DMA slot mapped by this driver's init and aligned for the access width; volatile stops the compiler eliding or tearing the access.
                unsafe { dst.add(i).write_volatile(*px) };
            }
        }
    }

    fn clear_cell(&mut self, col: usize, row: usize) {
        self.fill_rect(self.x0 + col * self.cell_w, self.y0 + row * self.cell_h, self.cell_w, self.cell_h, self.bg_color);
    }

    fn invert_cursor_cell(&mut self) {
        let ox = self.x0 + self.cursor_col * self.cell_w;
        let oy = self.y0 + self.cursor_row * self.cell_h;
        if self.cursor_col >= self.cols || self.cursor_row >= self.rows || ox + self.cell_w > self.width || oy + self.cell_h > self.height {
            return;
        }
        for py in 0..self.cell_h {
            // SAFETY: called at kernel privilege in the boot/driver order set up by kmain; the callee's own contract (hardware present, prerequisites initialized, interrupts masked where required) is met at this call site.
            let row = unsafe { self.base.add((oy + py) * self.pitch + ox * self.bytes_per_pixel) }.cast::<u32>();
            for px in 0..self.cell_w {
                // SAFETY: the address is a device register or ring/DMA slot mapped by this driver's init and aligned for the access width; volatile stops the compiler eliding or tearing the access.
                unsafe {
                    let p = row.add(px);
                    p.write_volatile(p.read_volatile() ^ 0x00FF_FFFF);
                }
            }
        }
    }

    fn hide_cursor(&mut self) {
        if self.cursor_drawn {
            self.invert_cursor_cell();
            self.cursor_drawn = false;
        }
    }

    fn show_cursor(&mut self) {
        if !self.cursor_drawn && self.cursor_enabled && self.blink_on && self.cols > 0 && self.rows > 0 {
            self.invert_cursor_cell();
            self.cursor_drawn = true;
        }
    }

    pub fn set_cursor_enabled(&mut self, on: bool) {
        self.cursor_enabled = on;
        if on {
            self.blink_on = true;
            self.show_cursor();
        } else {
            self.hide_cursor();
        }
    }

    pub fn blink(&mut self) {
        self.blink_on = !self.blink_on;
        if self.blink_on {
            self.show_cursor();
        } else {
            self.hide_cursor();
        }
    }

    pub fn clear(&mut self) {
        self.cursor_drawn = false;
        self.ansi_state = AnsiState::Normal;
        self.fg_color = 0xFFFFFFFF;
        self.bg_color = 0x00000000;
        self.fill_rect(0, 0, self.width, self.height, 0);
        self.cursor_col = 0;
        self.cursor_row = 0;
    }

    fn scroll(&mut self) {
        let row_bytes = self.cell_h * self.pitch;
        let total_bytes = self.rows * row_bytes;
        if total_bytes <= row_bytes {
            return;
        }
        // SAFETY: the pointer stays inside the framebuffer region handed to attach_framebuffer and the console has exclusive access.
        let top = unsafe { self.base.add(self.y0 * self.pitch) };
        // SAFETY: the pointer stays inside the framebuffer region handed to attach_framebuffer and the console has exclusive access.
        unsafe { copy_up(top, top.add(row_bytes), total_bytes - row_bytes) };
        let y = self.y0 + self.rows.saturating_sub(1) * self.cell_h;
        self.fill_rect(0, y, self.width, self.cell_h, self.bg_color);
    }

    fn newline(&mut self) {
        self.cursor_col = 0;
        if self.cursor_row + 1 >= self.rows {
            self.scroll();
        } else {
            self.cursor_row += 1;
        }
    }

    pub fn dims(&self) -> (usize, usize) {
        (self.cols, self.rows)
    }

    fn parse_cursor_sequence(&mut self, s: &str) -> bool {
        let Some(fin) = s.chars().last() else { return false };
        if !matches!(fin, 'A' | 'B' | 'C' | 'D' | 'H' | 'f' | 'J' | 'K') {
            return false;
        }
        let body = &s[..s.len() - 1];
        let mut nums = [None::<usize>; 2];
        for (i, t) in body.split(';').take(2).enumerate() {
            nums[i] = t.parse::<usize>().ok();
        }
        let n1 = nums[0].filter(|&n| n > 0).unwrap_or(1);
        let mode = nums[0].unwrap_or(0);
        match fin {
            'A' => self.cursor_row = self.cursor_row.saturating_sub(n1),
            'B' => self.cursor_row = (self.cursor_row + n1).min(self.rows - 1),
            'C' => self.cursor_col = (self.cursor_col + n1).min(self.cols - 1),
            'D' => self.cursor_col = self.cursor_col.saturating_sub(n1),
            'H' | 'f' => {
                let r = nums[0].filter(|&n| n > 0).unwrap_or(1);
                let c = nums[1].filter(|&n| n > 0).unwrap_or(1);
                self.cursor_row = (r - 1).min(self.rows - 1);
                self.cursor_col = (c - 1).min(self.cols - 1);
            }
            'K' => {
                let (from, to) = match mode {
                    1 => (0, self.cursor_col + 1),
                    2 => (0, self.cols),
                    _ => (self.cursor_col, self.cols),
                };
                for col in from..to {
                    self.clear_cell(col, self.cursor_row);
                }
            }
            'J' => match mode {
                2 | 3 => {
                    let (r, c) = (self.cursor_row, self.cursor_col);
                    self.clear();
                    self.cursor_row = r;
                    self.cursor_col = c;
                }
                1 => {
                    for row in 0..self.cursor_row {
                        for col in 0..self.cols {
                            self.clear_cell(col, row);
                        }
                    }
                    for col in 0..=self.cursor_col {
                        self.clear_cell(col, self.cursor_row);
                    }
                }
                _ => {
                    for col in self.cursor_col..self.cols {
                        self.clear_cell(col, self.cursor_row);
                    }
                    for row in self.cursor_row + 1..self.rows {
                        for col in 0..self.cols {
                            self.clear_cell(col, row);
                        }
                    }
                }
            },
            _ => {}
        }
        true
    }

    fn parse_ansi_sequence(&mut self) {
        if self.ansi_len == 0 {
            return;
        }

        let local = self.ansi_buffer;
        let s = match core::str::from_utf8(&local[0..self.ansi_len]) {
            Ok(valid_str) => valid_str,
            Err(_) => return,
        };

        if self.parse_cursor_sequence(s) {
            return;
        }

        if s.ends_with('m') {
            let params = s.trim_end_matches('m');
            if params.is_empty() || params == "0" {
                self.fg_color = 0xFFFFFFFF;
                self.bg_color = 0x00000000;
                return;
            }

            for token in params.split(';') {
                if let Ok(code) = token.parse::<u32>() {
                    match code {
                        0 => {
                            self.fg_color = 0xFFFFFFFF;
                            self.bg_color = 0x00000000;
                        }
                        1 => {}
                        30..=37 => {
                            self.fg_color = PALETTE[(code - 30) as usize];
                        }
                        39 => self.fg_color = 0xFFFFFFFF,
                        40..=47 => {
                            self.bg_color = PALETTE[(code - 40) as usize];
                        }
                        49 => self.bg_color = 0x00000000,
                        90..=97 => {
                            self.fg_color = PALETTE[(code - 90 + 8) as usize];
                        }
                        100..=107 => {
                            self.bg_color = PALETTE[(code - 100 + 8) as usize];
                        }
                        _ => {}
                    }
                }
            }
        }
    }

    pub fn write_char(&mut self, c: char) {
        if self.cols == 0 || self.rows == 0 {
            return;
        }

        match self.ansi_state {
            AnsiState::Escaped => {
                if c == '[' {
                    self.ansi_state = AnsiState::Csi;
                    self.ansi_len = 0;
                } else {
                    self.ansi_state = AnsiState::Normal;
                }
                return;
            }
            AnsiState::Csi => {
                if self.ansi_len < self.ansi_buffer.len() {
                    self.ansi_buffer[self.ansi_len] = c as u8;
                    self.ansi_len += 1;
                } else {
                    self.ansi_state = AnsiState::Normal;
                    return;
                }

                if (c >= 'A' && c <= 'Z') || (c >= 'a' && c <= 'z') {
                    self.parse_ansi_sequence();
                    self.ansi_state = AnsiState::Normal;
                }
                return;
            }
            AnsiState::Normal => {
                if c == '\x1b' {
                    self.ansi_state = AnsiState::Escaped;
                    return;
                }
            }
        }

        match c {
            '\n' => {
                self.newline();
            }
            '\r' => {
                self.cursor_col = 0;
            }
            '\x08' => {
                if self.cursor_col > 0 {
                    self.cursor_col -= 1;
                } else if self.cursor_row > 0 {
                    self.cursor_row -= 1;
                    self.cursor_col = self.cols - 1;
                }
                self.clear_cell(self.cursor_col, self.cursor_row);
            }
            '\t' => {
                let next_tab = (self.cursor_col + 4) & !3;
                while self.cursor_col < next_tab && self.cursor_col < self.cols {
                    self.clear_cell(self.cursor_col, self.cursor_row);
                    self.cursor_col += 1;
                }
                if self.cursor_col >= self.cols {
                    self.newline();
                }
            }
            _ => {
                if self.cursor_col >= self.cols {
                    self.newline();
                }
                self.draw_glyph(c, self.cursor_col, self.cursor_row);
                self.cursor_col += 1;
            }
        }
    }
}

impl fmt::Write for TextConsole {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        self.hide_cursor();
        for c in s.chars() {
            self.write_char(c);
        }
        self.blink_on = true;
        self.show_cursor();
        Ok(())
    }
}
