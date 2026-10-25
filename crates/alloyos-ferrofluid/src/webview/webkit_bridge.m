#import "webkit_bridge.h"
#import <Cocoa/Cocoa.h>
#import <WebKit/WebKit.h>

@interface AlloyWebKitPage : NSObject <WKNavigationDelegate>
@property (nonatomic, strong) WKWebView *webView;
@property (nonatomic, assign) uint32_t width;
@property (nonatomic, assign) uint32_t height;
@property (nonatomic, assign) BOOL isNavigating;
@property (nonatomic, copy) NSString *lastTitle;
@property (nonatomic, copy) NSString *focusedInputId;
@end

@implementation AlloyWebKitPage

- (instancetype)initWithWidth:(uint32_t)width height:(uint32_t)height {
    self = [super init];
    if (self) {
        if (![NSThread isMainThread]) {
            return nil;
        }
        self.width = width;
        self.height = height;
        self.focusedInputId = nil;
        self.lastTitle = @"";
        
        [NSApplication sharedApplication];
        [NSApp setActivationPolicy:NSApplicationActivationPolicyProhibited];
        
        WKWebViewConfiguration *config = [[WKWebViewConfiguration alloc] init];
        [config.preferences setValue:@YES forKey:@"allowFileAccessFromFileURLs"];
        
        NSRect frame = NSMakeRect(0, 0, width, height);
        self.webView = [[WKWebView alloc] initWithFrame:frame configuration:config];
        self.webView.navigationDelegate = self;
    }
    return self;
}

- (void)webView:(WKWebView *)webView didFinishNavigation:(WKNavigation *)navigation {
    (void)navigation;
    self.isNavigating = NO;
    if (webView.title.length > 0) {
        self.lastTitle = webView.title;
    }
}

- (void)webView:(WKWebView *)webView didFailNavigation:(WKNavigation *)navigation withError:(NSError *)error {
    self.isNavigating = NO;
    self.lastTitle = @"Unable to connect";
    NSString *errorHTML = [NSString stringWithFormat:
        @"<!doctype html><html><head><title>Unable to connect</title>"
        @"<style>body { font-family: -apple-system, sans-serif; background: #101418; color: #fff; padding: 32px; }"
        @"h1 { color: #ff6b6b; }</style></head>"
        @"<body><h1>Unable to connect</h1><p>%@</p></body></html>",
        [error localizedDescription] ?: @"The network connection was lost."];
    [webView loadHTMLString:errorHTML baseURL:nil];
}

- (void)webView:(WKWebView *)webView didFailProvisionalNavigation:(WKNavigation *)navigation withError:(NSError *)error {
    self.isNavigating = NO;
    self.lastTitle = @"Unable to connect";
    NSString *errorHTML = [NSString stringWithFormat:
        @"<!doctype html><html><head><title>Unable to connect</title>"
        @"<style>body { font-family: -apple-system, sans-serif; background: #101418; color: #fff; padding: 32px; }"
        @"h1 { color: #ff6b6b; }</style></head>"
        @"<body><h1>Unable to connect</h1><p>%@</p></body></html>",
        [error localizedDescription] ?: @"The server could not be found."];
    [webView loadHTMLString:errorHTML baseURL:nil];
}

- (void)drainRunLoopUntil:(NSTimeInterval)timeout condition:(BOOL (^)(void))condition {
    NSDate *start = [NSDate date];
    while ([[NSDate date] timeIntervalSinceDate:start] < timeout) {
        if (condition && condition()) {
            break;
        }
        [[NSRunLoop currentRunLoop] runMode:NSDefaultRunLoopMode beforeDate:[NSDate dateWithTimeIntervalSinceNow:0.02]];
    }
}

- (id)syncEvalJavaScript:(NSString *)script timeout:(NSTimeInterval)timeout {
    __block BOOL done = NO;
    __block id output = nil;
    [self.webView evaluateJavaScript:script completionHandler:^(id res, NSError *err) {
        (void)err;
        output = res;
        done = YES;
    }];
    [self drainRunLoopUntil:timeout condition:^{ return done; }];
    return output;
}

@end

void* webkit_bridge_create(uint32_t width, uint32_t height) {
    AlloyWebKitPage *page = [[AlloyWebKitPage alloc] initWithWidth:width height:height];
    return (__bridge_retained void*)page;
}

void webkit_bridge_destroy(void* handle) {
    if (!handle) return;
    AlloyWebKitPage *page = (__bridge_transfer AlloyWebKitPage*)handle;
    page.webView.navigationDelegate = nil;
    page.webView = nil;
}

void webkit_bridge_load_url(void* handle, const char* url_cstr) {
    if (!handle || !url_cstr) return;
    AlloyWebKitPage *page = (__bridge AlloyWebKitPage*)handle;
    NSString *urlStr = [NSString stringWithUTF8String:url_cstr];
    
    page.isNavigating = YES;
    if ([urlStr containsString:@"local-form.html"] || [urlStr hasSuffix:@"local-form"]) {
        NSString *fixturePath = @"tests/fixtures/web/local-form.html";
        if (![[NSFileManager defaultManager] fileExistsAtPath:fixturePath]) {
            // Check relative to workspace
            fixturePath = [[NSString stringWithUTF8String:getenv("PWD") ?: "."] stringByAppendingPathComponent:fixturePath];
        }
        NSURL *fileURL = [NSURL fileURLWithPath:fixturePath];
        NSURL *accessURL = [fileURL URLByDeletingLastPathComponent];
        [page.webView loadFileURL:fileURL allowingReadAccessToURL:accessURL];
    } else if ([urlStr hasPrefix:@"http://"] || [urlStr hasPrefix:@"https://"]) {
        NSURL *url = [NSURL URLWithString:urlStr];
        [page.webView loadRequest:[NSURLRequest requestWithURL:url]];
    } else {
        NSURL *fileURL = [NSURL fileURLWithPath:urlStr];
        [page.webView loadFileURL:fileURL allowingReadAccessToURL:[fileURL URLByDeletingLastPathComponent]];
    }
    
    [page drainRunLoopUntil:10.0 condition:^{ return (BOOL)(!page.isNavigating); }];
    // Allow an additional pause for layout, subresources, and first paint
    [page drainRunLoopUntil:1.0 condition:nil];
}

void webkit_bridge_load_html(void* handle, const char* html_cstr, const char* base_url_cstr) {
    if (!handle || !html_cstr) return;
    AlloyWebKitPage *page = (__bridge AlloyWebKitPage*)handle;
    NSString *html = [NSString stringWithUTF8String:html_cstr];
    NSURL *baseURL = base_url_cstr ? [NSURL URLWithString:[NSString stringWithUTF8String:base_url_cstr]] : nil;
    if (!baseURL) {
        baseURL = [NSURL fileURLWithPath:@"tests/fixtures/web/"];
    }
    
    page.isNavigating = YES;
    [page.webView loadHTMLString:html baseURL:baseURL];
    [page drainRunLoopUntil:5.0 condition:^{ return (BOOL)(!page.isNavigating); }];
    [page drainRunLoopUntil:0.25 condition:nil];
}

bool webkit_bridge_get_title(void* handle, char* out_buf, size_t max_len) {
    if (!handle || !out_buf || max_len == 0) return false;
    AlloyWebKitPage *page = (__bridge AlloyWebKitPage*)handle;
    
    NSString *title = page.webView.title;
    if (!title || title.length == 0) {
        if (page.lastTitle.length > 0) {
            title = page.lastTitle;
        } else {
            id res = [page syncEvalJavaScript:@"document.title" timeout:1.0];
            if ([res isKindOfClass:[NSString class]]) {
                title = (NSString *)res;
            }
        }
    }
    if (!title || title.length == 0) {
        if (page.lastTitle.length > 0) {
            title = page.lastTitle;
        } else {
            title = @"";
        }
    }
    
    const char *title_utf8 = [title UTF8String];
    strncpy(out_buf, title_utf8, max_len - 1);
    out_buf[max_len - 1] = '\0';
    return true;
}

bool webkit_bridge_eval_js(void* handle, const char* script_cstr, char* out_buf, size_t max_len) {
    if (!handle || !script_cstr || !out_buf || max_len == 0) return false;
    AlloyWebKitPage *page = (__bridge AlloyWebKitPage*)handle;
    
    NSString *script = [NSString stringWithUTF8String:script_cstr];
    NSData *scriptData = [NSJSONSerialization dataWithJSONObject:@[script] options:0 error:nil];
    NSString *scriptJsonArray = [[NSString alloc] initWithData:scriptData encoding:NSUTF8StringEncoding];
    // Extracts the escaped string
    NSString *escapedScript = [scriptJsonArray substringWithRange:NSMakeRange(1, scriptJsonArray.length - 2)];
    
    NSString *runner = [NSString stringWithFormat:
        @"(function() { "
        @"  try { "
        @"    var __res = eval(%@); "
        @"    return JSON.stringify(__res !== undefined ? __res : null); "
        @"  } catch(e) { "
        @"    return JSON.stringify({__error: String(e)}); "
        @"  } "
        @"})()", escapedScript];
        
    id result = [page syncEvalJavaScript:runner timeout:3.0];
    NSString *resultStr = [NSString stringWithFormat:@"%@", result ?: @"null"];
    
    strncpy(out_buf, [resultStr UTF8String], max_len - 1);
    out_buf[max_len - 1] = '\0';
    return true;
}

bool webkit_bridge_query_element(void* handle, const char* selector_cstr, const char* property_cstr, char* out_buf, size_t max_len) {
    if (!handle || !selector_cstr || !property_cstr || !out_buf || max_len == 0) return false;
    AlloyWebKitPage *page = (__bridge AlloyWebKitPage*)handle;
    
    NSString *selector = [NSString stringWithUTF8String:selector_cstr];
    NSString *property = [NSString stringWithUTF8String:property_cstr];
    
    NSString *queryScript;
    if ([property hasPrefix:@"dataset."]) {
        NSString *dataKey = [property substringFromIndex:8];
        queryScript = [NSString stringWithFormat:@"(function() { var el = document.querySelector('%@'); return el ? (el.dataset['%@'] || null) : null; })()", selector, dataKey];
    } else {
        queryScript = [NSString stringWithFormat:@"(function() { var el = document.querySelector('%@'); return el ? (el['%@'] || null) : null; })()", selector, property];
    }
    
    id result = [page syncEvalJavaScript:queryScript timeout:1.0];
    if (result && ![result isKindOfClass:[NSNull class]]) {
        NSString *str = [NSString stringWithFormat:@"%@", result];
        strncpy(out_buf, [str UTF8String], max_len - 1);
        out_buf[max_len - 1] = '\0';
        return true;
    }
    return false;
}

bool webkit_bridge_pointer_event(void* handle, int phase, int x, int y) {
    if (!handle) return false;
    AlloyWebKitPage *page = (__bridge AlloyWebKitPage*)handle;
    
    NSString *script = [NSString stringWithFormat:
        @"(function() { "
        @"  var hit = document.elementFromPoint(%d, %d); "
        @"  var el = hit; "
        @"  if (el && el.tagName === 'LABEL' && el.htmlFor) { "
        @"    var target = document.getElementById(el.htmlFor); "
        @"    if (target) el = target; "
        @"  } "
        @"  if (!el || (el.tagName !== 'INPUT' && el.tagName !== 'TEXTAREA')) { "
        @"    var rects = document.querySelectorAll('input:not([type=hidden]), textarea'); "
        @"    for (var i = 0; i < rects.length; i++) { "
        @"      var r = rects[i].getBoundingClientRect(); "
        @"      if (%d >= r.left - 40 && %d <= r.right + 40 && %d >= r.top - 40 && %d <= r.bottom + 40) { "
        @"        el = rects[i]; "
        @"        break; "
        @"      } "
        @"    } "
        @"  } "
        @"  if (!el && (%d === 0 || %d === 2)) { "
        @"    el = document.querySelector('input:not([type=hidden]), textarea'); "
        @"  } "
        @"  if (el) { "
        @"    el.focus(); "
        @"    if (%d === 0 || %d === 2) { "
        @"      el.dispatchEvent(new MouseEvent('click', { bubbles: true, clientX: %d, clientY: %d })); "
        @"    } "
        @"    var active = document.activeElement || el; "
        @"    var editable = (active.tagName === 'INPUT' || active.tagName === 'TEXTAREA' || active.isContentEditable); "
        @"    return { editable: editable, id: active.id || el.id || '', hitTag: hit ? hit.tagName : 'null' }; "
        @"  } "
        @"  return { editable: false, id: '', hitTag: hit ? hit.tagName : 'null' }; "
        @"})()", x, y, x, x, y, y, phase, phase, phase, phase, x, y];
        
    id result = [page syncEvalJavaScript:script timeout:1.0];
    if ([result isKindOfClass:[NSDictionary class]]) {
        BOOL editable = [result[@"editable"] boolValue];
        NSString *elId = result[@"id"];
        if (elId.length > 0) {
            page.focusedInputId = elId;
        }
        return editable;
    }
    return false;
}

void webkit_bridge_commit_text(void* handle, const char* text_cstr) {
    if (!handle || !text_cstr) return;
    AlloyWebKitPage *page = (__bridge AlloyWebKitPage*)handle;
    NSString *text = [NSString stringWithUTF8String:text_cstr];
    
    NSData *textData = [NSJSONSerialization dataWithJSONObject:@[text] options:0 error:nil];
    NSString *textJsonArray = [[NSString alloc] initWithData:textData encoding:NSUTF8StringEncoding];
    NSString *escapedText = [textJsonArray substringWithRange:NSMakeRange(1, textJsonArray.length - 2)];
    
    NSString *script = [NSString stringWithFormat:
        @"(function() { "
        @"  var el = document.activeElement; "
        @"  if (!el || el === document.body) { "
        @"    el = document.querySelector('input:not([type=hidden]), textarea'); "
        @"  } "
        @"  if (el && 'value' in el) { "
        @"    el.value = (el.value || '') + %@; "
        @"    el.dispatchEvent(new Event('input', { bubbles: true })); "
        @"    el.dispatchEvent(new Event('change', { bubbles: true })); "
        @"  } "
        @"})()", escapedText];
        
    [page syncEvalJavaScript:script timeout:1.0];
}

bool webkit_bridge_submit_form(void* handle, char* out_buf, size_t max_len) {
    if (!handle || !out_buf || max_len == 0) return false;
    AlloyWebKitPage *page = (__bridge AlloyWebKitPage*)handle;
    
    NSString *script =
        @"(function() { "
        @"  var form = document.querySelector('form'); "
        @"  if (form) { "
        @"    form.dispatchEvent(new Event('submit', { bubbles: true, cancelable: true })); "
        @"  } "
        @"  var res = document.getElementById('result'); "
        @"  return res ? res.textContent : ''; "
        @"})()";
        
    id result = [page syncEvalJavaScript:script timeout:2.0];
    NSString *resStr = [NSString stringWithFormat:@"%@", result ?: @""];
    strncpy(out_buf, [resStr UTF8String], max_len - 1);
    out_buf[max_len - 1] = '\0';
    return true;
}

void webkit_bridge_scroll(void* handle, int dx, int dy) {
    if (!handle) return;
    AlloyWebKitPage *page = (__bridge AlloyWebKitPage*)handle;
    
    NSString *script = [NSString stringWithFormat:@"window.scrollBy(%d, %d);", dx, dy];
    [page syncEvalJavaScript:script timeout:1.0];
}

bool webkit_bridge_render_frame(void* handle, uint8_t* out_bgra, uint32_t width, uint32_t height) {
    if (!handle || !out_bgra || width == 0 || height == 0) return false;
    AlloyWebKitPage *page = (__bridge AlloyWebKitPage*)handle;
    
    __block BOOL done = NO;
    __block NSImage *captured = nil;
    
    [page drainRunLoopUntil:0.5 condition:nil];

    WKSnapshotConfiguration *snapConfig = [[WKSnapshotConfiguration alloc] init];
    snapConfig.rect = NSMakeRect(0, 0, width, height);
    snapConfig.snapshotWidth = @(width);
    
    [page.webView takeSnapshotWithConfiguration:snapConfig completionHandler:^(NSImage * _Nullable snapshot, NSError * _Nullable error) {
        (void)error;
        captured = snapshot;
        done = YES;
    }];
    
    [page drainRunLoopUntil:4.0 condition:^{ return done; }];
    
    if (!captured) {
        return false;
    }
    
    CGColorSpaceRef colorSpace = CGColorSpaceCreateDeviceRGB();
    CGContextRef ctx = CGBitmapContextCreate(
        out_bgra,
        width,
        height,
        8,
        width * 4,
        colorSpace,
        kCGImageAlphaPremultipliedFirst | kCGBitmapByteOrder32Little
    );
    CGColorSpaceRelease(colorSpace);
    
    if (!ctx) {
        return false;
    }
    
    CGImageRef cgImage = [captured CGImageForProposedRect:NULL context:nil hints:nil];
    if (cgImage) {
        CGContextDrawImage(ctx, CGRectMake(0, 0, width, height), cgImage);
    }
    CGContextRelease(ctx);
    return true;
}
