#ifndef WEBKIT_BRIDGE_H
#define WEBKIT_BRIDGE_H

#include <stdint.h>
#include <stdbool.h>
#include <stddef.h>

#ifdef __cplusplus
extern "C" {
#endif

/// Creates an offscreen genuine WebKit page with specified dimensions.
void* webkit_bridge_create(uint32_t width, uint32_t height);

/// Destroys a WebKit page instance.
void webkit_bridge_destroy(void* handle);

/// Loads a URL (http, https, file, data) into WebKit.
void webkit_bridge_load_url(void* handle, const char* url);

/// Loads raw HTML markup with an optional base URL into WebKit.
void webkit_bridge_load_html(void* handle, const char* html, const char* base_url);

/// Retrieves the page title from WebKit into out_buf (null-terminated UTF-8).
bool webkit_bridge_get_title(void* handle, char* out_buf, size_t max_len);

/// Evaluates arbitrary JavaScript in the page's execution context.
bool webkit_bridge_eval_js(void* handle, const char* script, char* out_buf, size_t max_len);

/// Queries a property (e.g. "textContent", "value", "dataset.<name>", "tagName") on an element.
bool webkit_bridge_query_element(void* handle, const char* selector, const char* property, char* out_buf, size_t max_len);

/// Dispatches a pointer interaction event at (x, y).
/// Phase: 0 = Down, 1 = Move, 2 = Up.
/// Returns true if an editable text element is focused.
bool webkit_bridge_pointer_event(void* handle, int phase, int x, int y);

/// Commits text into the currently active/focused editable element.
void webkit_bridge_commit_text(void* handle, const char* text);

/// Submits the active or primary form on the page and returns the result text.
bool webkit_bridge_submit_form(void* handle, char* out_buf, size_t max_len);

/// Scrolls the page viewport by (dx, dy).
void webkit_bridge_scroll(void* handle, int dx, int dy);

/// Rasterizes the current WebKit page view into a BGRA8 pixel buffer (width * height * 4).
bool webkit_bridge_render_frame(void* handle, uint8_t* out_bgra, uint32_t width, uint32_t height);

#ifdef __cplusplus
}
#endif

#endif // WEBKIT_BRIDGE_H
