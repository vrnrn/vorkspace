#include "bridge.h"
#include <ghostty/vt.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>
#include <stdio.h>

struct Vt {
  GhosttyTerminal terminal;
  GhosttyRenderState render;
  GhosttyRenderStateRowIterator row;
  GhosttyRenderStateRowCells cells;
  GhosttyKeyEncoder key_encoder;
  GhosttyKeyEvent key_event;
  GhosttyTrackedGridRef anchor, anchor_end;
  uint32_t selection_kind;
  bool anchor_right;
  bool held;
  uint64_t hold_started_ms;
  uint16_t columns, rows;
  uint32_t cell_width, cell_height;
  uint32_t default_fg, default_bg, default_cursor;
  VtEffect effect;
  void *userdata;
};

static uint64_t now_ms(void) {
  struct timespec ts;
  clock_gettime(CLOCK_MONOTONIC, &ts);
  return (uint64_t)ts.tv_sec * 1000 + (uint64_t)ts.tv_nsec / 1000000;
}
static GhosttyColorRgb rgb(uint32_t n) {
  return (GhosttyColorRgb){ .r = n >> 16, .g = n >> 8, .b = n };
}
static uint32_t hex(GhosttyColorRgb c) { return (uint32_t)c.r << 16 | (uint32_t)c.g << 8 | c.b; }
static void write_pty(GhosttyTerminal t, void *userdata, const uint8_t *data, size_t len) {
  (void)t;
  Vt *v = userdata;
  v->effect(v->userdata, 0, data, len);
}
static void clipboard_write(GhosttyTerminal t, void *userdata, const GhosttyClipboardWrite *write) {
  (void)t;
  Vt *v = userdata;
  GhosttyClipboardWriteReply reply = GHOSTTY_INIT_SIZED(GhosttyClipboardWriteReply);
  reply.result = GHOSTTY_CLIPBOARD_WRITE_RESULT_UNSUPPORTED;
  if (write->location != GHOSTTY_CLIPBOARD_LOCATION_PRIMARY) {
    if (write->contents_len == 0) {
      v->effect(v->userdata, 1, NULL, 0);
      reply.result = GHOSTTY_CLIPBOARD_WRITE_RESULT_SUCCESS;
    }
    for (size_t i = 0; i < write->contents_len; i++) {
      const GhosttyClipboardContent *c = &write->contents[i];
      if (c->mime.len >= 10 && memcmp(c->mime.ptr, "text/plain", 10) == 0) {
        v->effect(v->userdata, 1, c->data.ptr, c->data.len);
        reply.result = GHOSTTY_CLIPBOARD_WRITE_RESULT_SUCCESS;
        break;
      }
    }
  }
  write->reply(write, &reply);
}
static bool terminal_size(GhosttyTerminal t, void *userdata, GhosttySizeReportSize *out) {
  (void)t;
  Vt *v = userdata;
  out->columns = v->columns;
  out->rows = v->rows;
  out->cell_width = v->cell_width;
  out->cell_height = v->cell_height;
  return true;
}
static bool device_attributes(GhosttyTerminal t, void *userdata, GhosttyDeviceAttributes *out) {
  (void)t; (void)userdata;
  out->primary.conformance_level = GHOSTTY_DA_CONFORMANCE_VT220;
  out->primary.features[0] = GHOSTTY_DA_FEATURE_ANSI_COLOR;
  out->primary.num_features = 1;
  out->secondary.device_type = GHOSTTY_DA_DEVICE_TYPE_VT220;
  return true;
}
static void render_hold(GhosttyTerminal t, void *userdata, bool held) {
  Vt *v = userdata;
  if (held) {
    ghostty_render_state_update(v->render, t);
    v->hold_started_ms = now_ms();
  }
  v->held = held;
}
static void release_hold(Vt *v) {
  if (!v->held) return;
  GhosttyTerminalModeConfig mode = { .mode = GHOSTTY_MODE_SYNC_OUTPUT, .value = false };
  ghostty_terminal_set(v->terminal, GHOSTTY_TERMINAL_OPT_MODE, &mode);
  v->held = false;
}
int vorkspace_vt_expire_hold(Vt *v) {
  if (v->held && now_ms() - v->hold_started_ms >= 1000) {
    release_hold(v);
    return 1;
  }
  return 0;
}
uint32_t vorkspace_vt_hold_remaining(Vt *v) {
  if (!v->held) return UINT32_MAX;
  uint64_t elapsed = now_ms() - v->hold_started_ms;
  return elapsed >= 1000 ? 0 : 1000 - elapsed;
}
int vorkspace_vt_default_colors(Vt *v, uint32_t fg, uint32_t bg, uint32_t cursor) {
  GhosttyColorRgb c;
  int r;
  if (v->default_fg != fg) {
    c = rgb(fg); r = ghostty_terminal_set(v->terminal, GHOSTTY_TERMINAL_OPT_COLOR_FOREGROUND, &c);
    if (r != GHOSTTY_SUCCESS) return r;
    v->default_fg = fg;
  }
  if (v->default_bg != bg) {
    c = rgb(bg); r = ghostty_terminal_set(v->terminal, GHOSTTY_TERMINAL_OPT_COLOR_BACKGROUND, &c);
    if (r != GHOSTTY_SUCCESS) return r;
    v->default_bg = bg;
  }
  if (v->default_cursor != cursor) {
    c = rgb(cursor); r = ghostty_terminal_set(v->terminal, GHOSTTY_TERMINAL_OPT_COLOR_CURSOR, &c);
    if (r != GHOSTTY_SUCCESS) return r;
    v->default_cursor = cursor;
  }
  return GHOSTTY_SUCCESS;
}
Vt *vorkspace_vt_new(uint16_t cols, uint16_t rows, size_t history_lines, size_t history_bytes,
                     const uint32_t *palette, uint32_t fg, uint32_t bg, uint32_t cursor,
                     VtEffect effect, void *userdata) {
  Vt *v = calloc(1, sizeof(*v));
  if (!v) return NULL;
  v->effect = effect;
  v->userdata = userdata;
  v->columns = cols; v->rows = rows; v->cell_width = 8; v->cell_height = 16;
  if (ghostty_terminal_new(NULL, &v->terminal, cols, rows) != GHOSTTY_SUCCESS ||
      ghostty_render_state_new(NULL, &v->render) != GHOSTTY_SUCCESS ||
      ghostty_render_state_row_iterator_new(NULL, &v->row) != GHOSTTY_SUCCESS ||
      ghostty_render_state_row_cells_new(NULL, &v->cells) != GHOSTTY_SUCCESS ||
      ghostty_key_encoder_new(NULL, &v->key_encoder) != GHOSTTY_SUCCESS ||
      ghostty_key_event_new(NULL, &v->key_event) != GHOSTTY_SUCCESS) goto fail;
#define SET(key, value) if (ghostty_terminal_set(v->terminal, key, value) != GHOSTTY_SUCCESS) goto fail
  SET(GHOSTTY_TERMINAL_OPT_USERDATA, v);
  SET(GHOSTTY_TERMINAL_OPT_WRITE_PTY, write_pty);
  SET(GHOSTTY_TERMINAL_OPT_CLIPBOARD_WRITE, clipboard_write);
  SET(GHOSTTY_TERMINAL_OPT_SIZE, terminal_size);
  SET(GHOSTTY_TERMINAL_OPT_DEVICE_ATTRIBUTES, device_attributes);
  SET(GHOSTTY_TERMINAL_OPT_RENDER_HOLD, render_hold);
  SET(GHOSTTY_TERMINAL_OPT_SCROLLBACK_MAX_LINES, &history_lines);
  SET(GHOSTTY_TERMINAL_OPT_SCROLLBACK_MAX_BYTES, &history_bytes);
  size_t max_clipboard = 1024 * 1024;
  SET(GHOSTTY_TERMINAL_OPT_CLIPBOARD_WRITE_MAX_BYTES, &max_clipboard);
  v->default_fg = fg; v->default_bg = bg; v->default_cursor = cursor;
  GhosttyColorRgb color = rgb(fg);
  SET(GHOSTTY_TERMINAL_OPT_COLOR_FOREGROUND, &color);
  color = rgb(bg); SET(GHOSTTY_TERMINAL_OPT_COLOR_BACKGROUND, &color);
  color = rgb(cursor); SET(GHOSTTY_TERMINAL_OPT_COLOR_CURSOR, &color);
  GhosttyColorRgb colors[256];
  for (size_t i = 0; i < 256; i++) colors[i] = rgb(palette[i]);
  SET(GHOSTTY_TERMINAL_OPT_COLOR_PALETTE, colors);
  GhosttyTerminalCursorStyle style = GHOSTTY_TERMINAL_CURSOR_STYLE_BAR;
  SET(GHOSTTY_TERMINAL_OPT_DEFAULT_CURSOR_STYLE, &style);
  bool no = false;
  SET(GHOSTTY_TERMINAL_OPT_GLYPH_PROTOCOL, &no);
#undef SET
  // Apply the configured default cursor style to the initial active screen.
  ghostty_terminal_reset(v->terminal);
  return v;
fail:
  vorkspace_vt_free(v);
  return NULL;
}
void vorkspace_vt_free(Vt *v) {
  if (!v) return;
  ghostty_tracked_grid_ref_free(v->anchor);
  ghostty_tracked_grid_ref_free(v->anchor_end);
  ghostty_key_event_free(v->key_event);
  ghostty_key_encoder_free(v->key_encoder);
  ghostty_render_state_row_cells_free(v->cells);
  ghostty_render_state_row_iterator_free(v->row);
  ghostty_render_state_free(v->render);
  ghostty_terminal_free(v->terminal);
  free(v);
}
void vorkspace_vt_write(Vt *v, const uint8_t *bytes, size_t len) {
  ghostty_terminal_vt_write(v->terminal, bytes, len);
}
int vorkspace_vt_resize(Vt *v, uint16_t cols, uint16_t rows, uint32_t cw, uint32_t ch) {
  // A size-query callback fired by resize must observe the new geometry.
  uint16_t old_cols = v->columns, old_rows = v->rows;
  uint32_t old_cw = v->cell_width, old_ch = v->cell_height;
  v->columns = cols; v->rows = rows; v->cell_width = cw; v->cell_height = ch;
  GhosttyResult r = ghostty_terminal_resize(v->terminal, cols, rows, cw, ch);
  if (r != GHOSTTY_SUCCESS) {
    v->columns = old_cols; v->rows = old_rows; v->cell_width = old_cw; v->cell_height = old_ch;
  }
  return r;
}
static bool mode(Vt *v, GhosttyMode m) {
  GhosttyTerminalModeConfig c = { .mode = m };
  return ghostty_terminal_get(v->terminal, GHOSTTY_TERMINAL_DATA_MODE, &c) == GHOSTTY_SUCCESS && c.value;
}
uint32_t vorkspace_vt_modes(Vt *v) {
  uint32_t bits = 0;
  if (mode(v, GHOSTTY_MODE_DECCKM)) bits |= 1;
  if (mode(v, GHOSTTY_MODE_BRACKETED_PASTE)) bits |= 2;
  if (mode(v, GHOSTTY_MODE_ALT_SCROLL)) bits |= 4;
  GhosttyTerminalScreen screen = GHOSTTY_TERMINAL_SCREEN_PRIMARY;
  ghostty_terminal_get(v->terminal, GHOSTTY_TERMINAL_DATA_ACTIVE_SCREEN, &screen);
  if (screen == GHOSTTY_TERMINAL_SCREEN_ALTERNATE) bits |= 8;
  return bits;
}
uint64_t vorkspace_vt_display_offset(Vt *v) {
  GhosttyTerminalScrollbar s = {0};
  ghostty_terminal_get(v->terminal, GHOSTTY_TERMINAL_DATA_SCROLLBAR, &s);
  return s.total > s.offset + s.len ? s.total - s.offset - s.len : 0;
}
void vorkspace_vt_scroll(Vt *v, int32_t delta, uint32_t bottom) {
  release_hold(v);
  GhosttyTerminalScrollViewport scroll = {0};
  scroll.tag = bottom ? GHOSTTY_SCROLL_VIEWPORT_BOTTOM : GHOSTTY_SCROLL_VIEWPORT_DELTA;
  scroll.value.delta = -(intptr_t)delta;
  ghostty_terminal_scroll_viewport(v->terminal, scroll);
}
int vorkspace_vt_frame(Vt *v, VtFrame *out, VtPaintCell paint, void *userdata) {
  vorkspace_vt_expire_hold(v);
  GhosttyResult r = v->held ? GHOSTTY_SUCCESS : ghostty_render_state_update(v->render, v->terminal);
  if (r != GHOSTTY_SUCCESS) return r;
  GhosttyRenderStateColors colors = GHOSTTY_INIT_SIZED(GhosttyRenderStateColors);
  GhosttyRenderStateCursor cursor = GHOSTTY_INIT_SIZED(GhosttyRenderStateCursor);
  ghostty_render_state_get(v->render, GHOSTTY_RENDER_STATE_DATA_COLORS, &colors);
  ghostty_render_state_get(v->render, GHOSTTY_RENDER_STATE_DATA_CURSOR, &cursor);
  *out = (VtFrame){ .columns = v->columns, .rows = v->rows,
    .cursor_visible = cursor.visible && cursor.viewport_has_value,
    .cursor_shape = cursor.visual_style, .foreground = hex(colors.foreground),
    .background = hex(colors.background),
    .cursor = colors.cursor_has_value ? hex(colors.cursor) : hex(colors.foreground) };
  if (out->cursor_visible) { out->cursor_x = cursor.viewport_x; out->cursor_y = cursor.viewport_y; }
  ghostty_render_state_get(v->render, GHOSTTY_RENDER_STATE_DATA_ROW_ITERATOR, &v->row);
  uint8_t local[128];
  uint8_t *text = local;
  size_t capacity = sizeof(local);
  uint32_t y = 0;
  while (ghostty_render_state_row_iterator_next(v->row)) {
    ghostty_render_state_row_get(v->row, GHOSTTY_RENDER_STATE_ROW_DATA_CELLS, &v->cells);
    uint32_t x = 0;
    while (ghostty_render_state_row_cells_next(v->cells)) {
      GhosttyCell raw;
      GhosttyCellWide wide = GHOSTTY_CELL_WIDE_NARROW;
      ghostty_render_state_row_cells_get(v->cells, GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_RAW, &raw);
      ghostty_cell_get(raw, GHOSTTY_CELL_DATA_WIDE, &wide);
      uint32_t column = x++;
      if (wide == GHOSTTY_CELL_WIDE_SPACER_TAIL || wide == GHOSTTY_CELL_WIDE_SPACER_HEAD) continue;
      GhosttyStyle style = {0};
      ghostty_render_state_row_cells_get(v->cells, GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_STYLE, &style);
      GhosttyColorRgb fg = colors.foreground, bg = colors.background;
      bool fg_set = ghostty_render_state_row_cells_get(v->cells, GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_FG_COLOR, &fg) == GHOSTTY_SUCCESS;
      bool bg_set = ghostty_render_state_row_cells_get(v->cells, GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_BG_COLOR, &bg) == GHOSTTY_SUCCESS;
      bool selected = false;
      ghostty_render_state_row_cells_get(v->cells, GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_SELECTED, &selected);
      GhosttyBuffer buffer = { .ptr = text, .cap = capacity };
      r = ghostty_render_state_row_cells_get(v->cells, GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_GRAPHEMES_UTF8, &buffer);
      if (r == GHOSTTY_OUT_OF_SPACE) {
        uint8_t *grown = malloc(buffer.len);
        if (!grown) { if (text != local) free(text); return GHOSTTY_OUT_OF_MEMORY; }
        if (text != local) free(text);
        text = grown; capacity = buffer.len;
        buffer = (GhosttyBuffer){ .ptr = text, .cap = capacity };
        r = ghostty_render_state_row_cells_get(v->cells, GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_GRAPHEMES_UTF8, &buffer);
      }
      if (r != GHOSTTY_SUCCESS) { if (text != local) free(text); return r; }
      VtCell cell = { .row = y, .column = column, .width = wide == GHOSTTY_CELL_WIDE_WIDE ? 2 : 1,
        .foreground = hex(fg), .background = hex(bg), .foreground_set = fg_set,
        .background_set = bg_set, .selected = selected,
        .flags = style.flags };
      paint(userdata, &cell, text, buffer.len);
    }
    y++;
  }
  if (text != local) free(text);
  return ghostty_render_state_clean(v->render);
}

static GhosttyPoint point_at(uint16_t x, uint32_t y, GhosttyPointTag tag) {
  GhosttyPoint p = { .tag = tag, .value = { .coordinate = { .x = x, .y = y } } };
  return p;
}
static uint64_t index_at(GhosttyPointCoordinate p, uint16_t cols) { return (uint64_t)p.y * cols + p.x; }
static int track_selection(Vt *v, const GhosttySelection *s) {
  GhosttyPointCoordinate a, b;
  if (ghostty_terminal_point_from_grid_ref(v->terminal, &s->start, GHOSTTY_POINT_TAG_SCREEN, &a) != GHOSTTY_SUCCESS ||
      ghostty_terminal_point_from_grid_ref(v->terminal, &s->end, GHOSTTY_POINT_TAG_SCREEN, &b) != GHOSTTY_SUCCESS) return GHOSTTY_NO_VALUE;
  GhosttyTrackedGridRef start = NULL, end = NULL;
  int r = ghostty_terminal_grid_ref_track(v->terminal, point_at(a.x, a.y, GHOSTTY_POINT_TAG_SCREEN), &start);
  if (r == GHOSTTY_SUCCESS) r = ghostty_terminal_grid_ref_track(v->terminal, point_at(b.x, b.y, GHOSTTY_POINT_TAG_SCREEN), &end);
  if (r != GHOSTTY_SUCCESS) { ghostty_tracked_grid_ref_free(start); return r; }
  ghostty_tracked_grid_ref_free(v->anchor); ghostty_tracked_grid_ref_free(v->anchor_end);
  v->anchor = start; v->anchor_end = end;
  return GHOSTTY_SUCCESS;
}
static int anchor_selection(Vt *v, GhosttySelection *out) {
  GhosttyPointCoordinate a, b;
  if (ghostty_tracked_grid_ref_point(v->anchor, GHOSTTY_POINT_TAG_SCREEN, &a) != GHOSTTY_SUCCESS ||
      ghostty_tracked_grid_ref_point(v->anchor_end, GHOSTTY_POINT_TAG_SCREEN, &b) != GHOSTTY_SUCCESS) return GHOSTTY_NO_VALUE;
  int r = ghostty_terminal_grid_ref(v->terminal, point_at(a.x, a.y, GHOSTTY_POINT_TAG_SCREEN), &out->start);
  if (r != GHOSTTY_SUCCESS) return r;
  r = ghostty_terminal_grid_ref(v->terminal, point_at(b.x, b.y, GHOSTTY_POINT_TAG_SCREEN), &out->end);
  out->rectangle = false;
  return r;
}
void vorkspace_vt_clear_selection(Vt *v) {
  ghostty_terminal_set(v->terminal, GHOSTTY_TERMINAL_OPT_SELECTION, NULL);
  ghostty_tracked_grid_ref_free(v->anchor); ghostty_tracked_grid_ref_free(v->anchor_end);
  v->anchor = NULL; v->anchor_end = NULL;
}
int vorkspace_vt_has_selection(Vt *v) {
  GhosttySelection s;
  return ghostty_terminal_get(v->terminal, GHOSTTY_TERMINAL_DATA_SELECTION, &s) == GHOSTTY_SUCCESS;
}
static int selection_at(Vt *v, GhosttyGridRef ref, uint32_t kind, GhosttySelection *out) {
  if (kind == 1) {
    GhosttyTerminalSelectWordOptions o = GHOSTTY_INIT_SIZED(GhosttyTerminalSelectWordOptions); o.ref = ref;
    return ghostty_terminal_select_word(v->terminal, &o, out);
  }
  if (kind == 2) {
    GhosttyTerminalSelectLineOptions o = GHOSTTY_INIT_SIZED(GhosttyTerminalSelectLineOptions); o.ref = ref;
    return ghostty_terminal_select_line(v->terminal, &o, out);
  }
  *out = (GhosttySelection){ .start = ref, .end = ref };
  return GHOSTTY_SUCCESS;
}
int vorkspace_vt_select(Vt *v, uint16_t x, uint16_t y, uint32_t right, uint32_t kind, uint32_t extend) {
  release_hold(v);
  GhosttyGridRef ref;
  int r = ghostty_terminal_grid_ref(v->terminal, point_at(x, y, GHOSTTY_POINT_TAG_VIEWPORT), &ref);
  if (r != GHOSTTY_SUCCESS) return r;
  GhosttySelection next;
  if (!extend || !v->anchor) {
    r = selection_at(v, ref, kind, &next);
    if (r != GHOSTTY_SUCCESS) return r;
    r = track_selection(v, &next);
    if (r != GHOSTTY_SUCCESS) return r;
    r = anchor_selection(v, &next);
    if (r != GHOSTTY_SUCCESS) return r;
    v->anchor_right = right; v->selection_kind = kind;
    if (kind == 0) {
      ghostty_terminal_set(v->terminal, GHOSTTY_TERMINAL_OPT_SELECTION, NULL);
      return GHOSTTY_SUCCESS;
    }
  } else {
    GhosttyPointCoordinate a, b, p;
    if (ghostty_tracked_grid_ref_point(v->anchor, GHOSTTY_POINT_TAG_SCREEN, &a) != GHOSTTY_SUCCESS ||
        ghostty_tracked_grid_ref_point(v->anchor_end, GHOSTTY_POINT_TAG_SCREEN, &b) != GHOSTTY_SUCCESS ||
        ghostty_terminal_point_from_grid_ref(v->terminal, &ref, GHOSTTY_POINT_TAG_SCREEN, &p) != GHOSTTY_SUCCESS) {
      vorkspace_vt_clear_selection(v); return GHOSTTY_NO_VALUE;
    }
    if (v->selection_kind == 0) {
      uint64_t anchor = index_at(a, v->columns) + v->anchor_right;
      uint64_t end = index_at(p, v->columns) + right;
      if (anchor == end) { ghostty_terminal_set(v->terminal, GHOSTTY_TERMINAL_OPT_SELECTION, NULL); return GHOSTTY_SUCCESS; }
      uint64_t lo = anchor < end ? anchor : end, hi = (anchor > end ? anchor : end) - 1;
      if (ghostty_terminal_grid_ref(v->terminal, point_at(lo % v->columns, lo / v->columns, GHOSTTY_POINT_TAG_SCREEN), &next.start) != GHOSTTY_SUCCESS ||
          ghostty_terminal_grid_ref(v->terminal, point_at(hi % v->columns, hi / v->columns, GHOSTTY_POINT_TAG_SCREEN), &next.end) != GHOSTTY_SUCCESS) return GHOSTTY_NO_VALUE;
      next.rectangle = false;
    } else {
      r = selection_at(v, ref, v->selection_kind, &next);
      if (r != GHOSTTY_SUCCESS) return r;
      if (index_at(p, v->columns) < index_at(a, v->columns)) {
        ghostty_terminal_grid_ref(v->terminal, point_at(b.x, b.y, GHOSTTY_POINT_TAG_SCREEN), &next.end);
      } else {
        ghostty_terminal_grid_ref(v->terminal, point_at(a.x, a.y, GHOSTTY_POINT_TAG_SCREEN), &next.start);
      }
    }
  }
  return ghostty_terminal_set(v->terminal, GHOSTTY_TERMINAL_OPT_SELECTION, &next);
}
int vorkspace_vt_select_all(Vt *v) {
  release_hold(v);
  GhosttySelection s;
  int r = ghostty_terminal_select_all(v->terminal, &s);
  if (r != GHOSTTY_SUCCESS) return r;
  r = track_selection(v, &s);
  if (r != GHOSTTY_SUCCESS) return r;
  r = anchor_selection(v, &s);
  if (r != GHOSTTY_SUCCESS) return r;
  v->selection_kind = 2;
  return ghostty_terminal_set(v->terminal, GHOSTTY_TERMINAL_OPT_SELECTION, &s);
}
int vorkspace_vt_selection_text(Vt *v, uint8_t *buf, size_t cap, size_t *len) {
  GhosttyTerminalSelectionFormatOptions o = GHOSTTY_INIT_SIZED(GhosttyTerminalSelectionFormatOptions);
  o.emit = GHOSTTY_FORMATTER_FORMAT_PLAIN; o.unwrap = true; o.trim = true;
  return ghostty_terminal_selection_format_buf(v->terminal, &o, buf, cap, len);
}

// Keep enum values inside C: the Rust boundary passes GPUI's logical key name.
static GhosttyKey key_named(const uint8_t *name, size_t len) {
  if (len == 1) {
    uint8_t c = name[0];
    if (c >= 'A' && c <= 'Z') c += 'a' - 'A';
    if (c >= 'a' && c <= 'z') return GHOSTTY_KEY_A + c - 'a';
    if (c >= '0' && c <= '9') return GHOSTTY_KEY_DIGIT_0 + c - '0';
    switch (c) {
      case '[': return GHOSTTY_KEY_BRACKET_LEFT; case ']': return GHOSTTY_KEY_BRACKET_RIGHT;
      case '\\': return GHOSTTY_KEY_BACKSLASH; case '/': return GHOSTTY_KEY_SLASH;
      case '-': return GHOSTTY_KEY_MINUS; case '=': return GHOSTTY_KEY_EQUAL;
      case ',': return GHOSTTY_KEY_COMMA; case '.': return GHOSTTY_KEY_PERIOD;
      case ';': return GHOSTTY_KEY_SEMICOLON; case '\'': return GHOSTTY_KEY_QUOTE;
      case '`': return GHOSTTY_KEY_BACKQUOTE; case ' ': return GHOSTTY_KEY_SPACE;
    }
  }
  if (len >= 2 && len <= 3 && name[0] == 'f') {
    unsigned n = 0;
    for (size_t i = 1; i < len; i++) {
      if (name[i] < '0' || name[i] > '9') { n = 0; break; }
      n = n * 10 + name[i] - '0';
    }
    if (n >= 1 && n <= 25) return GHOSTTY_KEY_F1 + n - 1;
  }
#define KEY(name_string, value) if (len == sizeof(name_string) - 1 && memcmp(name, name_string, len) == 0) return value
  KEY("up", GHOSTTY_KEY_ARROW_UP); KEY("down", GHOSTTY_KEY_ARROW_DOWN);
  KEY("left", GHOSTTY_KEY_ARROW_LEFT); KEY("right", GHOSTTY_KEY_ARROW_RIGHT);
  KEY("home", GHOSTTY_KEY_HOME); KEY("end", GHOSTTY_KEY_END);
  KEY("pageup", GHOSTTY_KEY_PAGE_UP); KEY("pagedown", GHOSTTY_KEY_PAGE_DOWN);
  KEY("insert", GHOSTTY_KEY_INSERT); KEY("delete", GHOSTTY_KEY_DELETE);
  KEY("enter", GHOSTTY_KEY_ENTER); KEY("backspace", GHOSTTY_KEY_BACKSPACE);
  KEY("tab", GHOSTTY_KEY_TAB); KEY("escape", GHOSTTY_KEY_ESCAPE); KEY("space", GHOSTTY_KEY_SPACE);
#undef KEY
  return GHOSTTY_KEY_UNIDENTIFIED;
}
int vorkspace_vt_key(Vt *v, const uint8_t *name, size_t name_len, uint32_t mods,
                    uint32_t codepoint, uint32_t action, const uint8_t *text, size_t text_len,
                    uint8_t *out, size_t capacity, size_t *written) {
  ghostty_key_encoder_setopt_from_terminal(v->key_encoder, v->terminal);
  GhosttyOptionAsAlt option = GHOSTTY_OPTION_AS_ALT_TRUE;
  ghostty_key_encoder_setopt(v->key_encoder, GHOSTTY_KEY_ENCODER_OPT_MACOS_OPTION_AS_ALT, &option);
  ghostty_key_event_set_action(v->key_event, action);
  ghostty_key_event_set_key(v->key_event, key_named(name, name_len));
  ghostty_key_event_set_mods(v->key_event, mods);
  ghostty_key_event_set_consumed_mods(v->key_event, 0);
  ghostty_key_event_set_utf8(v->key_event, (const char *)text, text_len);
  ghostty_key_event_set_unshifted_codepoint(v->key_event, codepoint);
  int result = ghostty_key_encoder_encode(v->key_encoder, v->key_event, (char *)out, capacity, written);
  // The reusable event must not retain a Rust text borrow after this call.
  ghostty_key_event_set_utf8(v->key_event, NULL, 0);
  // Keep the existing C0 editing keys in default legacy mode. Explicit Kitty
  // and modifyOtherKeys modes retain the encoder's richer representations.
  GhosttyKittyKeyFlags kitty = 0;
  ghostty_terminal_get(v->terminal, GHOSTTY_TERMINAL_DATA_KITTY_KEYBOARD_FLAGS, &kitty);
  if (result == GHOSTTY_SUCCESS && kitty == 0 && (mods & GHOSTTY_MODS_CTRL) &&
      (codepoint == '[' || codepoint == 'i' || codepoint == 'm')) {
    char expected[32];
    unsigned modifier = 1 + !!(mods & GHOSTTY_MODS_SHIFT) + 2 * !!(mods & GHOSTTY_MODS_ALT) + 4;
    int len = snprintf(expected, sizeof(expected), "\033[%u;%uu", codepoint, modifier);
    if (len > 0 && *written == (size_t)len && memcmp(out, expected, *written) == 0) {
      *written = 0;
      if (mods & GHOSTTY_MODS_ALT) out[(*written)++] = 27;
      out[(*written)++] = codepoint == '[' ? 27 : codepoint == 'i' ? 9 : 13;
    }
  }
  return result;
}
