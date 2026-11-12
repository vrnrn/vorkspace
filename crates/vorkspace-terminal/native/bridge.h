#ifndef VORKSPACE_GHOSTTY_BRIDGE_H
#define VORKSPACE_GHOSTTY_BRIDGE_H
#include <stddef.h>
#include <stdint.h>

typedef struct Vt Vt;
typedef void (*VtEffect)(void *, uint32_t, const uint8_t *, size_t);
typedef struct {
  uint32_t row, column, width, foreground, background, flags;
  uint32_t foreground_set, background_set, selected;
} VtCell;
typedef struct {
  uint32_t columns, rows, cursor_x, cursor_y, cursor_shape, cursor_visible;
  uint32_t foreground, background, cursor;
} VtFrame;
typedef void (*VtPaintCell)(void *, const VtCell *, const uint8_t *, size_t);

Vt *vorkspace_vt_new(uint16_t, uint16_t, size_t, size_t, const uint32_t *,
                    uint32_t, uint32_t, uint32_t, VtEffect, void *);
void vorkspace_vt_free(Vt *);
void vorkspace_vt_write(Vt *, const uint8_t *, size_t);
int vorkspace_vt_resize(Vt *, uint16_t, uint16_t, uint32_t, uint32_t);
int vorkspace_vt_frame(Vt *, VtFrame *, VtPaintCell, void *);
uint32_t vorkspace_vt_modes(Vt *);
uint64_t vorkspace_vt_display_offset(Vt *);
void vorkspace_vt_scroll(Vt *, int32_t, uint32_t);
int vorkspace_vt_select(Vt *, uint16_t, uint16_t, uint32_t, uint32_t, uint32_t);
int vorkspace_vt_select_all(Vt *);
void vorkspace_vt_clear_selection(Vt *);
int vorkspace_vt_selection_text(Vt *, uint8_t *, size_t, size_t *);
int vorkspace_vt_expire_hold(Vt *);
uint32_t vorkspace_vt_hold_remaining(Vt *);
int vorkspace_vt_default_colors(Vt *, uint32_t, uint32_t, uint32_t);
int vorkspace_vt_key(Vt *, const uint8_t *, size_t, uint32_t, uint32_t, uint32_t,
                    const uint8_t *, size_t, uint8_t *, size_t, size_t *);
#endif
