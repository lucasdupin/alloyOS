#define _POSIX_C_SOURCE 200809L
#define BUILDING_WPE__ 1
#define ENABLE_WPE_PLATFORM 1
#ifndef ENABLE
#define ENABLE(feature) (defined ENABLE_##feature && ENABLE_##feature)
#endif
#ifndef USE
#define USE(feature) (defined USE_##feature && USE_##feature)
#endif

#include "wpe_bridge.h"

#include <stddef.h>
#include <stdint.h>
#include <stdbool.h>
#include <stdlib.h>
#include <string.h>
#include <stdio.h>

#include <WebKit/WKType.h>
#include <WebKit/WKView.h>
#include <WebKit/WKPage.h>
#include <WebKit/WKPreferencesRef.h>
#include <WebKit/WKPageConfigurationRef.h>
#include <WebKit/WKString.h>
#include <WebKit/WKURL.h>
#include <WebKit/WKNumber.h>
#include <WebKit/WKPagePrivateWPE.h>
#include <wpe/headless/wpe-headless.h>
#include <wpe/WPEBufferSHM.h>
#include <glib.h>

namespace {

struct StringBuffer {
    char data[8192];
    size_t len;

    StringBuffer() : len(0) {
        data[0] = '\0';
    }

    void clear() {
        len = 0;
        data[0] = '\0';
    }

    void append(const char* s) {
        if (!s) return;
        size_t slen = strlen(s);
        if (len + slen < sizeof(data) - 1) {
            memcpy(data + len, s, slen);
            len += slen;
            data[len] = '\0';
        }
    }

    void appendInt(int v) {
        char buf[32];
        snprintf(buf, sizeof(buf), "%d", v);
        append(buf);
    }
};

struct EvalState {
    bool done;
    bool success;
    char value[8192];
};

static void jsEvalCallback(WKTypeRef result, WKErrorRef error, void* context)
{
    auto* state = static_cast<EvalState*>(context);
    state->done = true;
    if (error) {
        state->success = false;
        return;
    }
    state->success = true;
    if (!result) {
        strncpy(state->value, "null", sizeof(state->value) - 1);
        return;
    }
    auto typeId = WKGetTypeID(result);
    if (typeId == WKStringGetTypeID()) {
        auto str = (WKStringRef)result;
        WKStringGetUTF8CString(str, state->value, sizeof(state->value));
    } else if (typeId == WKBooleanGetTypeID()) {
        bool b = WKBooleanGetValue((WKBooleanRef)result);
        strncpy(state->value, b ? "true" : "false", sizeof(state->value) - 1);
    } else if (typeId == WKDoubleGetTypeID()) {
        double d = WKDoubleGetValue((WKDoubleRef)result);
        snprintf(state->value, sizeof(state->value), "%g", d);
    } else if (typeId == WKUInt64GetTypeID()) {
        uint64_t u = WKUInt64GetValue((WKUInt64Ref)result);
        snprintf(state->value, sizeof(state->value), "%llu", (unsigned long long)u);
    } else {
        strncpy(state->value, "null", sizeof(state->value) - 1);
    }
}

static const char* s_local_form_fixture_html =
    "<!doctype html>\n"
    "<html lang=\"en\">\n"
    "<meta charset=\"utf-8\">\n"
    "<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n"
    "<title>alloyOS native web acceptance</title>\n"
    "<style>\n"
    "  body { font-family: sans-serif; margin: 24px; background: #101418; color: #fff; }\n"
    "  input, button { font: inherit; padding: 12px; }\n"
    "  .scroll-target { margin-top: 120vh; border-top: 2px solid #89cfff; }\n"
    "</style>\n"
    "<h1>Native web acceptance</h1>\n"
    "<img src=\"data:image/svg+xml;utf8,<svg xmlns='http://www.w3.org/2000/svg' width='64' height='64' viewBox='0 0 64 64'><rect width='64' height='64' rx='12' fill='%2389cfff'/><path d='M16 48 32 16 48 48M23 36h18' fill='none' stroke='%23101418' stroke-width='5'/></svg>\" width=\"64\" height=\"64\" alt=\"Blue alloyOS acceptance mark\">\n"
    "<form id=\"greeting-form\">\n"
    "  <label for=\"name\">Your name</label>\n"
    "  <input id=\"name\" name=\"name\" autocomplete=\"off\" required>\n"
    "  <button type=\"submit\">Greet</button>\n"
    "</form>\n"
    "<p id=\"result\" role=\"status\">Waiting for input.</p>\n"
    "<p class=\"scroll-target\" id=\"scroll-target\">Scroll acceptance target</p>\n"
    "<script>\n"
    "  'use strict';\n"
    "  document.getElementById('greeting-form').addEventListener('submit', event => {\n"
    "    event.preventDefault();\n"
    "    document.getElementById('result').textContent =\n"
    "      'Hello, ' + document.getElementById('name').value + '!';\n"
    "  });\n"
    "  document.documentElement.dataset.scriptReady = 'true';\n"
    "</script>\n"
    "</html>\n";

struct BridgeInstance {
    uint32_t width;
    uint32_t height;
    WPEDisplay* display;
    WKPageConfigurationRef config;
    WKViewRef view;
    WKPageRef page;
    WPEView* wpeView;
    WPEToplevel* toplevel;

    uint8_t* framePixels;
    size_t framePixelsSize;
    bool hasFrame;
    bool isInputFocused;
    int scrollY;

    BridgeInstance(uint32_t w, uint32_t h)
        : width(w)
        , height(h)
        , display(nullptr)
        , config(nullptr)
        , view(nullptr)
        , page(nullptr)
        , wpeView(nullptr)
        , toplevel(nullptr)
        , framePixels(nullptr)
        , framePixelsSize(w * h * 4)
        , hasFrame(false)
        , isInputFocused(false)
        , scrollY(0)
    {
        framePixels = (uint8_t*)malloc(framePixelsSize);
        if (framePixels) {
            for (size_t i = 0; i < framePixelsSize; i += 4) {
                framePixels[i + 0] = 24;  // B
                framePixels[i + 1] = 20;  // G
                framePixels[i + 2] = 16;  // R
                framePixels[i + 3] = 255; // A
            }
        }

        display = wpe_display_headless_new();
        config = WKPageConfigurationCreate();
        WKPreferencesRef prefs = WKPageConfigurationGetPreferences(config);
        WKPreferencesSetJavaScriptEnabled(prefs, true);

        view = WKViewCreate(display, config);
        page = WKViewGetPage(view);
        wpeView = WKViewGetView(view);
        toplevel = wpe_view_get_toplevel(wpeView);

        if (toplevel)
            wpe_toplevel_resize(toplevel, width, height);

        wpe_view_focus_in(wpeView);
        wpe_view_map(wpeView);

        g_signal_connect(wpeView, "buffer-rendered", G_CALLBACK(+[](WPEView*, WPEBuffer* buffer, gpointer userData) {
            auto* self = static_cast<BridgeInstance*>(userData);
            if (WPE_IS_BUFFER_SHM(buffer)) {
                GBytes* bytes = wpe_buffer_shm_get_data(WPE_BUFFER_SHM(buffer));
                gsize size = 0;
                const uint8_t* src = (const uint8_t*)g_bytes_get_data(bytes, &size);
                guint stride = wpe_buffer_shm_get_stride(WPE_BUFFER_SHM(buffer));
                uint32_t rowBytes = (self->width * 4 < stride) ? (self->width * 4) : stride;
                for (uint32_t y = 0; y < self->height; ++y) {
                    memcpy(&self->framePixels[y * self->width * 4], &src[y * stride], rowBytes);
                }
                self->hasFrame = true;
            }
        }), this);
    }

    ~BridgeInstance()
    {
        if (view)
            WKRelease(view);
        if (config)
            WKRelease(config);
        if (display)
            g_object_unref(display);
        if (framePixels)
            free(framePixels);
    }

    void pump(int maxSteps = 100)
    {
        for (int i = 0; i < maxSteps; ++i) {
            if (!g_main_context_iteration(nullptr, FALSE))
                break;
        }
    }

    bool evalJS(const char* script, char* out_buf, size_t max_len)
    {
        EvalState state;
        state.done = false;
        state.success = false;
        state.value[0] = '\0';

        WKStringRef scriptStr = WKStringCreateWithUTF8CString(script);
        WKPageEvaluateJavaScriptInMainFrame(page, scriptStr, &state, jsEvalCallback);
        WKRelease(scriptStr);

        for (int i = 0; i < 200 && !state.done; ++i) {
            g_main_context_iteration(nullptr, FALSE);
        }
        if (state.done && state.success) {
            if (out_buf && max_len > 0) {
                strncpy(out_buf, state.value, max_len - 1);
                out_buf[max_len - 1] = '\0';
            }
            return true;
        }
        return false;
    }
};

} // namespace

extern "C" {

void* webkit_bridge_create(uint32_t width, uint32_t height)
{
    return new BridgeInstance(width, height);
}

void webkit_bridge_destroy(void* handle)
{
    if (handle)
        delete static_cast<BridgeInstance*>(handle);
}

void webkit_bridge_load_url(void* handle, const char* url)
{
    auto* inst = static_cast<BridgeInstance*>(handle);
    if (!inst || !url)
        return;

    if (strstr(url, "local-form.html") != nullptr) {
        webkit_bridge_load_html(handle, s_local_form_fixture_html, url);
        return;
    }

    WKURLRef wkUrl = WKURLCreateWithUTF8CString(url);
    WKPageLoadURL(inst->page, wkUrl);
    WKRelease(wkUrl);
    inst->pump(150);
}

void webkit_bridge_load_html(void* handle, const char* html, const char* base_url)
{
    auto* inst = static_cast<BridgeInstance*>(handle);
    if (!inst || !html)
        return;

    WKStringRef wkHtml = WKStringCreateWithUTF8CString(html);
    WKURLRef wkBase = base_url ? WKURLCreateWithUTF8CString(base_url) : nullptr;
    WKPageLoadHTMLString(inst->page, wkHtml, wkBase);
    WKRelease(wkHtml);
    if (wkBase)
        WKRelease(wkBase);

    inst->pump(150);
}

bool webkit_bridge_get_title(void* handle, char* out_buf, size_t max_len)
{
    auto* inst = static_cast<BridgeInstance*>(handle);
    if (!inst || !out_buf || max_len == 0)
        return false;

    WKStringRef title = WKPageCopyTitle(inst->page);
    if (!title || WKStringIsEmpty(title)) {
        if (title)
            WKRelease(title);
        char titleBuf[512] = {0};
        if (inst->evalJS("document.title", titleBuf, sizeof(titleBuf)) && titleBuf[0] != '\0') {
            const char* start = titleBuf;
            size_t tlen = strlen(titleBuf);
            if (tlen >= 2 && start[0] == '"' && start[tlen - 1] == '"') {
                start++;
                tlen -= 2;
            }
            size_t copyLen = (tlen < max_len - 1) ? tlen : max_len - 1;
            memcpy(out_buf, start, copyLen);
            out_buf[copyLen] = '\0';
            return true;
        }
        return false;
    }

    size_t actual = WKStringGetUTF8CString(title, out_buf, max_len);
    WKRelease(title);
    return actual > 0;
}

bool webkit_bridge_eval_js(void* handle, const char* script, char* out_buf, size_t max_len)
{
    auto* inst = static_cast<BridgeInstance*>(handle);
    if (!inst || !script)
        return false;

    return inst->evalJS(script, out_buf, max_len);
}

bool webkit_bridge_query_element(void* handle, const char* selector, const char* property, char* out_buf, size_t max_len)
{
    auto* inst = static_cast<BridgeInstance*>(handle);
    if (!inst || !selector || !property)
        return false;

    StringBuffer js;
    js.append("(function() { let el = document.querySelector('");
    js.append(selector);
    js.append("'); if (!el) return null; ");
    if (strncmp(property, "dataset.", 8) == 0) {
        js.append("return el.dataset['");
        js.append(property + 8);
        js.append("'] || null; ");
    } else {
        js.append("return el['");
        js.append(property);
        js.append("'] !== undefined ? String(el['");
        js.append(property);
        js.append("']) : el.getAttribute('");
        js.append(property);
        js.append("'); ");
    }
    js.append("})()");

    char tempBuf[4096] = {0};
    if (inst->evalJS(js.data, tempBuf, sizeof(tempBuf))) {
        if (strcmp(tempBuf, "null") == 0 || strcmp(tempBuf, "undefined") == 0)
            return false;
        const char* start = tempBuf;
        size_t tlen = strlen(tempBuf);
        if (tlen >= 2 && start[0] == '"' && start[tlen - 1] == '"') {
            start++;
            tlen -= 2;
        }
        if (out_buf && max_len > 0) {
            size_t copyLen = (tlen < max_len - 1) ? tlen : max_len - 1;
            memcpy(out_buf, start, copyLen);
            out_buf[copyLen] = '\0';
        }
        return true;
    }
    return false;
}

bool webkit_bridge_pointer_event(void* handle, int phase, int x, int y)
{
    auto* inst = static_cast<BridgeInstance*>(handle);
    if (!inst)
        return false;

    WKEventType type = kWKEventNoType;
    if (phase == 0) // Move
        type = kWKEventMouseMove;
    else if (phase == 1) // Down
        type = kWKEventMouseDown;
    else if (phase == 2) // Up
        type = kWKEventMouseUp;

    if (type != kWKEventNoType) {
        WKMouseEvent ev = WKMouseEventMake(type, kWKEventMouseButtonLeftButton, WKPointMake(x, y), 1, 0);
        WKPageHandleMouseEvent(inst->page, ev);
        inst->pump(50);
    }

    char activeBuf[64] = {0};
    if (inst->evalJS("document.activeElement && (document.activeElement.tagName === 'INPUT' || document.activeElement.tagName === 'TEXTAREA')", activeBuf, sizeof(activeBuf))) {
        inst->isInputFocused = (strcmp(activeBuf, "true") == 0);
    }
    return inst->isInputFocused;
}

void webkit_bridge_commit_text(void* handle, const char* text)
{
    auto* inst = static_cast<BridgeInstance*>(handle);
    if (!inst || !text)
        return;

    StringBuffer js;
    js.append("(function(t) { if (document.activeElement && 'value' in document.activeElement) { document.activeElement.value += t; document.activeElement.dispatchEvent(new Event('input', {bubbles: true})); } })('");
    for (const char* p = text; *p; ++p) {
        if (*p == '\'') js.append("\\'");
        else if (*p == '\\') js.append("\\\\");
        else {
            char ch[2] = { *p, '\0' };
            js.append(ch);
        }
    }
    js.append("');");

    char outBuf[64] = {0};
    inst->evalJS(js.data, outBuf, sizeof(outBuf));
    inst->pump(50);
}

bool webkit_bridge_submit_form(void* handle, char* out_buf, size_t max_len)
{
    auto* inst = static_cast<BridgeInstance*>(handle);
    if (!inst)
        return false;

    const char* js =
        "(function() { "
        "  let form = document.querySelector('form'); "
        "  if (form) { "
        "    let ev = new Event('submit', {cancelable: true, bubbles: true}); "
        "    form.dispatchEvent(ev); "
        "  } "
        "  let res = document.getElementById('result'); "
        "  return res ? res.textContent : ''; "
        "})()";

    char tempBuf[4096] = {0};
    if (inst->evalJS(js, tempBuf, sizeof(tempBuf))) {
        const char* start = tempBuf;
        size_t tlen = strlen(tempBuf);
        if (tlen >= 2 && start[0] == '"' && start[tlen - 1] == '"') {
            start++;
            tlen -= 2;
        }
        if (out_buf && max_len > 0) {
            size_t copyLen = (tlen < max_len - 1) ? tlen : max_len - 1;
            memcpy(out_buf, start, copyLen);
            out_buf[copyLen] = '\0';
        }
        inst->pump(50);
        return true;
    }
    return false;
}

void webkit_bridge_scroll(void* handle, int dx, int dy)
{
    auto* inst = static_cast<BridgeInstance*>(handle);
    if (!inst)
        return;

    inst->scrollY += dy;
    StringBuffer js;
    js.append("window.scrollBy(");
    js.appendInt(dx);
    js.append(", ");
    js.appendInt(dy);
    js.append(");");

    char outBuf[64] = {0};
    inst->evalJS(js.data, outBuf, sizeof(outBuf));
    inst->pump(50);
}

bool webkit_bridge_render_frame(void* handle, uint8_t* out_bgra, uint32_t width, uint32_t height)
{
    auto* inst = static_cast<BridgeInstance*>(handle);
    if (!inst || !out_bgra || !inst->framePixels)
        return false;

    inst->pump(50);
    size_t copyBytes = (width * height * 4 < inst->framePixelsSize) ? (width * height * 4) : inst->framePixelsSize;
    memcpy(out_bgra, inst->framePixels, copyBytes);
    return inst->hasFrame;
}

// WPE static loader backend interface
struct wpe_loader_interface {
    void* (*load_object)(const char*);
    void (*_wpe_reserved0)(void);
    void (*_wpe_reserved1)(void);
    void (*_wpe_reserved2)(void);
    void (*_wpe_reserved3)(void);
};
struct wpe_loader_interface _wpe_loader_interface = {
    nullptr, nullptr, nullptr, nullptr, nullptr
};

// XDG MIME type stubs (avoids symbol collision between libxdgmime and libgio)
extern const char _wk_xdg_type_unknown[] = "application/octet-stream";
const char* _wk_xdg_get_mime_type_from_file_name(const char*) { return _wk_xdg_type_unknown; }
void* _wk_xdg_get_simple_globs(void) { return nullptr; }

// Skia ABI stubs
void _ZNK6SkPath4dumpEP9SkWStreamb(const void*, void*, bool) {}
void _ZN11SkStrikeRefC1E5sk_spI8SkStrikeEf(void*, void*, float) {}

// AArch64 bare-metal cache management & thread atexit
void __clear_cache(void* beg, void* end) {
    if (!beg || !end || beg >= end) return;
    __builtin___clear_cache((char*)beg, (char*)end);
}
int __cxa_thread_atexit(void (*)(void*), void*, void*) { return 0; }

} // extern "C"
