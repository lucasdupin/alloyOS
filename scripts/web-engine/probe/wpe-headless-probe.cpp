#define BUILDING_WPE__ 1
#define ENABLE_WPE_PLATFORM 1
#ifndef ENABLE
#define ENABLE(feature) (defined ENABLE_##feature && ENABLE_##feature)
#endif
#ifndef USE
#define USE(feature) (defined USE_##feature && USE_##feature)
#endif

#include <WebKit/WKView.h>
#include <WebKit/WKPage.h>
#include <WebKit/WKPreferencesRef.h>
#include <WebKit/WKPageConfigurationRef.h>
#include <WebKit/WKString.h>
#include <WebKit/WKURL.h>
#include <WebKit/WKPagePrivateWPE.h>
#include <wpe/headless/wpe-headless.h>
#include <wpe/WPEBufferSHM.h>
#include <glib.h>
#include <stdio.h>

static int g_frames_rendered = 0;

static void on_buffer_rendered(WPEView* view, WPEBuffer* buffer, gpointer user_data)
{
    g_frames_rendered++;
    printf("[probe] buffer rendered! frame #%d\n", g_frames_rendered);
    if (WPE_IS_BUFFER_SHM(buffer)) {
        GBytes* bytes = wpe_buffer_shm_get_data(WPE_BUFFER_SHM(buffer));
        gsize size = 0;
        const void* data = g_bytes_get_data(bytes, &size);
        printf("[probe] SHM buffer data=%p, size=%lu, stride=%u\n",
               data, (unsigned long)size, wpe_buffer_shm_get_stride(WPE_BUFFER_SHM(buffer)));
    }
}

int main(int argc, char** argv)
{
    printf("[probe] Initializing WPE Headless display...\n");
    WPEDisplay* display = wpe_display_headless_new();
    if (!display) {
        printf("[probe] Failed to create headless display\n");
        return 1;
    }

    printf("[probe] Creating page configuration...\n");
    WKPageConfigurationRef config = WKPageConfigurationCreate();
    WKPreferencesRef prefs = WKPageConfigurationGetPreferences(config);
    WKPreferencesSetJavaScriptEnabled(prefs, true);

    printf("[probe] Creating WKView...\n");
    WKViewRef view = WKViewCreate(display, config);
    if (!view) {
        printf("[probe] Failed to create WKView\n");
        return 2;
    }

    WKPageRef page = WKViewGetPage(view);
    WPEView* wpeView = WKViewGetView(view);
    printf("[probe] WKView created! page=%p, wpeView=%p\n", (void*)page, (void*)wpeView);

    g_signal_connect(wpeView, "buffer-rendered", G_CALLBACK(on_buffer_rendered), NULL);

    printf("[probe] Loading HTML...\n");
    WKStringRef html = WKStringCreateWithUTF8CString("<html><body style='background:#101418;color:#fff;'><h1>Hello AlloyOS</h1></body></html>");
    WKURLRef url = WKURLCreateWithUTF8CString("https://local.alloyos.test/");
    WKPageLoadHTMLString(page, html, url);

    printf("[probe] Pumping main loop iterations...\n");
    for (int i = 0; i < 50; ++i) {
        g_main_context_iteration(NULL, FALSE);
    }

    printf("[probe] Done! Frames rendered: %d\n", g_frames_rendered);
    return 0;
}
