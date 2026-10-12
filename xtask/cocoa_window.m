#import <Cocoa/Cocoa.h>

// Run inside QEMU so resizing needs no Accessibility or Automation permissions.
__attribute__((constructor)) static void fit_phone_window(void)
{
    dispatch_async(dispatch_get_main_queue(), ^{
        __block unsigned attempts = 0;
        [NSTimer scheduledTimerWithTimeInterval:0.25 repeats:YES block:^(NSTimer *timer) {
            if (++attempts > 80) {
                [timer invalidate];
                return;
            }
            for (NSWindow *window in NSApp.windows) {
                NSView *view = window.contentView;
                if (!window.visible || ![NSStringFromClass(view.class) isEqualToString:@"QemuCocoaView"])
                    continue;
                NSSize guest = view.bounds.size;
                if (guest.height <= guest.width)
                    continue; // Wait until the guest switches to portrait scanout.
                NSRect available = (window.screen ?: NSScreen.mainScreen).visibleFrame;
                NSRect content = [window contentRectForFrameRect:window.frame];
                CGFloat chrome = window.frame.size.height - content.size.height;
                CGFloat ratio = guest.width / guest.height;
                CGFloat height = MIN(content.size.height * 4, available.size.height - chrome - 24);
                height = MIN(height, (available.size.width - 24) / ratio);
                [window setContentSize:NSMakeSize(height * ratio, height)];
                NSRect frame = window.frame;
                frame.origin.x = available.origin.x + (available.size.width - frame.size.width) / 2;
                frame.origin.y = available.origin.y + (available.size.height - frame.size.height) / 2;
                [window setFrame:frame display:YES];
                [timer invalidate];
                return;
            }
        }];
    });
}
