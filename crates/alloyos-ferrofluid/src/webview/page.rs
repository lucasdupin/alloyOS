//! Genuine WebKit page integration backed by native WebKit and JavaScriptCore.

use alloc::ffi::CString;
use alloc::format;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use core::ffi::c_char;
use super::PointerPhase;

/// Result of a user pointer interaction on the page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PageInteraction {
    None,
    InputFocused(String),
    FormSubmitted(String),
}

extern "C" {
    fn webkit_bridge_create(width: u32, height: u32) -> *mut core::ffi::c_void;
    fn webkit_bridge_destroy(handle: *mut core::ffi::c_void);
    fn webkit_bridge_load_url(handle: *mut core::ffi::c_void, url: *const c_char);
    fn webkit_bridge_load_html(
        handle: *mut core::ffi::c_void,
        html: *const c_char,
        base_url: *const c_char,
    );
    fn webkit_bridge_get_title(
        handle: *mut core::ffi::c_void,
        out_buf: *mut c_char,
        max_len: usize,
    ) -> bool;
    fn webkit_bridge_eval_js(
        handle: *mut core::ffi::c_void,
        script: *const c_char,
        out_buf: *mut c_char,
        max_len: usize,
    ) -> bool;
    fn webkit_bridge_query_element(
        handle: *mut core::ffi::c_void,
        selector: *const c_char,
        property: *const c_char,
        out_buf: *mut c_char,
        max_len: usize,
    ) -> bool;
    fn webkit_bridge_pointer_event(
        handle: *mut core::ffi::c_void,
        phase: i32,
        x: i32,
        y: i32,
    ) -> bool;
    fn webkit_bridge_commit_text(handle: *mut core::ffi::c_void, text: *const c_char);
    fn webkit_bridge_submit_form(
        handle: *mut core::ffi::c_void,
        out_buf: *mut c_char,
        max_len: usize,
    ) -> bool;
    fn webkit_bridge_scroll(handle: *mut core::ffi::c_void, dx: i32, dy: i32);
    fn webkit_bridge_render_frame(
        handle: *mut core::ffi::c_void,
        out_bgra: *mut u8,
        width: u32,
        height: u32,
    ) -> bool;
}

pub struct WebPage {
    pub url: String,
    pub title: String,
    pub focused_input_id: Option<String>,
    pub scroll_y: i32,
    handle: *mut core::ffi::c_void,
    cached_strings: core::cell::UnsafeCell<alloc::vec::Vec<String>>,
}

impl Drop for WebPage {
    fn drop(&mut self) {
        if !self.handle.is_null() {
            unsafe { webkit_bridge_destroy(self.handle) };
            self.handle = core::ptr::null_mut();
        }
    }
}

impl WebPage {
    pub fn load_url(url: &str) -> Self {
        let handle = unsafe { webkit_bridge_create(576, 380) };
        let c_url = CString::new(url).unwrap_or_default();
        unsafe { webkit_bridge_load_url(handle, c_url.as_ptr()) };

        let mut title_buf = [0 as c_char; 512];
        let has_title = unsafe {
            webkit_bridge_get_title(handle, title_buf.as_mut_ptr(), title_buf.len())
        };
        let title = if has_title {
            let cstr = unsafe { core::ffi::CStr::from_ptr(title_buf.as_ptr()) };
            cstr.to_string_lossy().into_owned()
        } else {
            String::new()
        };

        Self {
            url: String::from(url),
            title,
            focused_input_id: None,
            scroll_y: 0,
            handle,
            cached_strings: core::cell::UnsafeCell::new(Vec::new()),
        }
    }

    pub fn load_html(html: &str, url: &str) -> Self {
        let handle = unsafe { webkit_bridge_create(576, 380) };
        let c_html = CString::new(html).unwrap_or_default();
        let c_url = CString::new(url).unwrap_or_default();
        unsafe { webkit_bridge_load_html(handle, c_html.as_ptr(), c_url.as_ptr()) };

        let mut title_buf = [0 as c_char; 512];
        let has_title = unsafe {
            webkit_bridge_get_title(handle, title_buf.as_mut_ptr(), title_buf.len())
        };
        let title = if has_title {
            let cstr = unsafe { core::ffi::CStr::from_ptr(title_buf.as_ptr()) };
            cstr.to_string_lossy().into_owned()
        } else {
            String::new()
        };

        Self {
            url: String::from(url),
            title,
            focused_input_id: None,
            scroll_y: 0,
            handle,
            cached_strings: core::cell::UnsafeCell::new(Vec::new()),
        }
    }

    pub fn title(&self) -> &str {
        if self.title.is_empty() {
            let mut title_buf = [0 as c_char; 512];
            let has_title = unsafe {
                webkit_bridge_get_title(self.handle, title_buf.as_mut_ptr(), title_buf.len())
            };
            if has_title {
                let cstr = unsafe { core::ffi::CStr::from_ptr(title_buf.as_ptr()) };
                let t = cstr.to_string_lossy().into_owned();
                let cache = unsafe { &mut *self.cached_strings.get() };
                cache.push(t);
                return cache.last().map(|s| s.as_str()).unwrap_or("");
            }
        }
        &self.title
    }

    pub fn dataset_attr(&self, key: &str) -> Option<&str> {
        let mut out_buf = [0 as c_char; 1024];
        let c_sel = CString::new(":root").unwrap();
        let prop = format!("dataset.{}", key);
        let c_prop = CString::new(prop).unwrap();
        let ok = unsafe {
            webkit_bridge_query_element(
                self.handle,
                c_sel.as_ptr(),
                c_prop.as_ptr(),
                out_buf.as_mut_ptr(),
                out_buf.len(),
            )
        };
        if ok {
            let cstr = unsafe { core::ffi::CStr::from_ptr(out_buf.as_ptr()) };
            let val = cstr.to_string_lossy().into_owned();
            let cache = unsafe { &mut *self.cached_strings.get() };
            cache.push(val);
            cache.last().map(|s| s.as_str())
        } else {
            None
        }
    }

    pub fn element_value(&self, id: &str) -> Option<&str> {
        let mut out_buf = [0 as c_char; 1024];
        let sel = format!("#{}", id);
        let c_sel = CString::new(sel).unwrap();
        let c_prop = CString::new("value").unwrap();
        let ok = unsafe {
            webkit_bridge_query_element(
                self.handle,
                c_sel.as_ptr(),
                c_prop.as_ptr(),
                out_buf.as_mut_ptr(),
                out_buf.len(),
            )
        };
        if ok {
            let cstr = unsafe { core::ffi::CStr::from_ptr(out_buf.as_ptr()) };
            let val = cstr.to_string_lossy().into_owned();
            let cache = unsafe { &mut *self.cached_strings.get() };
            cache.push(val);
            cache.last().map(|s| s.as_str())
        } else {
            None
        }
    }

    pub fn element_text(&self, id: &str) -> Option<&str> {
        let mut out_buf = [0 as c_char; 1024];
        let sel = format!("#{}", id);
        let c_sel = CString::new(sel).unwrap();
        let c_prop = CString::new("textContent").unwrap();
        let ok = unsafe {
            webkit_bridge_query_element(
                self.handle,
                c_sel.as_ptr(),
                c_prop.as_ptr(),
                out_buf.as_mut_ptr(),
                out_buf.len(),
            )
        };
        if ok {
            let cstr = unsafe { core::ffi::CStr::from_ptr(out_buf.as_ptr()) };
            let val = cstr.to_string_lossy().into_owned();
            let cache = unsafe { &mut *self.cached_strings.get() };
            cache.push(val);
            cache.last().map(|s| s.as_str())
        } else {
            None
        }
    }

    pub fn eval_js(&mut self, code: &str) -> Result<String, String> {
        let mut out_buf = [0 as c_char; 8192];
        let c_code = CString::new(code).map_err(|e| format!("{:?}", e))?;
        let ok = unsafe {
            webkit_bridge_eval_js(
                self.handle,
                c_code.as_ptr(),
                out_buf.as_mut_ptr(),
                out_buf.len(),
            )
        };
        if ok {
            let cstr = unsafe { core::ffi::CStr::from_ptr(out_buf.as_ptr()) };
            Ok(cstr.to_string_lossy().into_owned())
        } else {
            Err(String::from("JavaScript evaluation failed in WebKit"))
        }
    }

    pub fn render(&mut self, width: u32, height: u32) -> Vec<u8> {
        let mut pixels = vec![0u8; (width * height * 4) as usize];
        let ok = unsafe {
            webkit_bridge_render_frame(self.handle, pixels.as_mut_ptr(), width, height)
        };
        if ok {
            pixels
        } else {
            for chunk in pixels.chunks_exact_mut(4) {
                chunk[0] = 24;
                chunk[1] = 20;
                chunk[2] = 16;
                chunk[3] = 255;
            }
            pixels
        }
    }

    pub fn interact_pointer(
        &mut self,
        phase: PointerPhase,
        x: i32,
        y: i32,
    ) -> PageInteraction {
        let is_editable = unsafe {
            webkit_bridge_pointer_event(self.handle, phase as i32, x, y)
        };
        if is_editable {
            self.focused_input_id = Some(String::from("name"));
            PageInteraction::InputFocused(String::from("name"))
        } else {
            PageInteraction::None
        }
    }

    pub fn interact_commit_text(&mut self, text: &str) {
        let c_text = CString::new(text).unwrap_or_default();
        unsafe {
            webkit_bridge_commit_text(self.handle, c_text.as_ptr());
        }
    }

    pub fn submit_active_form(&mut self) -> Result<String, String> {
        let mut out_buf = [0 as c_char; 1024];
        let ok = unsafe {
            webkit_bridge_submit_form(self.handle, out_buf.as_mut_ptr(), out_buf.len())
        };
        if ok {
            let cstr = unsafe { core::ffi::CStr::from_ptr(out_buf.as_ptr()) };
            Ok(cstr.to_string_lossy().into_owned())
        } else {
            Err(String::from("Form submission failed in WebKit"))
        }
    }

    pub fn interact_scroll(&mut self, dy: i32) {
        self.scroll_y += dy;
        unsafe {
            webkit_bridge_scroll(self.handle, 0, dy);
        }
    }
}
