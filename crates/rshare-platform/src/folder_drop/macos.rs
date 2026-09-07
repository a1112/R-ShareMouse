use anyhow::{bail, Context, Result};
use cocoa::appkit::{NSDragPboard, NSPasteboardURLReadingFileURLsOnlyKey};
use cocoa::{
    base::{id, nil},
    foundation::{NSArray, NSAutoreleasePool, NSString},
};
use core_foundation::{
    array::{CFArrayGetCount, CFArrayGetTypeID, CFArrayGetValueAtIndex},
    base::{CFGetTypeID, CFRelease, CFTypeRef, TCFType},
    string::CFString,
    url::CFURLGetTypeID,
};
use objc::{class, msg_send, sel, sel_impl};
use std::{ffi::CStr, path::PathBuf};

#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    fn AXUIElementCreateSystemWide() -> CFTypeRef;
    fn AXValueGetValue(value: CFTypeRef, kind: u32, result: *mut std::ffi::c_void) -> bool;
    fn AXValueGetTypeID() -> usize;
    fn AXUIElementCopyElementAtPosition(
        element: CFTypeRef,
        x: f32,
        y: f32,
        result: *mut CFTypeRef,
    ) -> i32;
    fn AXUIElementCopyAttributeValue(
        element: CFTypeRef,
        attribute: CFTypeRef,
        result: *mut CFTypeRef,
    ) -> i32;
    fn AXUIElementGetPid(element: CFTypeRef, pid: *mut i32) -> i32;
    fn AXUIElementSetMessagingTimeout(element: CFTypeRef, timeout: f32) -> i32;
}

struct Value(CFTypeRef);
impl Drop for Value {
    fn drop(&mut self) {
        unsafe {
            CFRelease(self.0);
        }
    }
}
impl Value {
    fn attribute(&self, name: &str) -> Option<Value> {
        let name = CFString::new(name);
        let mut value = std::ptr::null();
        let status =
            unsafe { AXUIElementCopyAttributeValue(self.0, name.as_CFTypeRef(), &mut value) };
        if status == 0 && !value.is_null() {
            Some(Value(value))
        } else {
            None
        }
    }
    fn string(&self) -> Option<String> {
        if unsafe { CFGetTypeID(self.0) } != CFString::type_id() {
            return None;
        }
        Some(unsafe { CFString::wrap_under_get_rule(self.0.cast()) }.to_string())
    }
    fn file_path(&self) -> Option<PathBuf> {
        unsafe {
            let url: id = if CFGetTypeID(self.0) == CFURLGetTypeID() {
                self.0 as id
            } else {
                let text = self.string()?;
                let text = ns_string(&text);
                msg_send![class!(NSURL), URLWithString: text]
            };
            if url == nil {
                return None;
            }
            let local: bool = msg_send![url, isFileURL];
            if !local {
                return None;
            }
            let path: id = msg_send![url, path];
            string(path).map(PathBuf::from)
        }
    }

    fn item_path(&self, depth: usize, budget: &mut usize) -> Option<PathBuf> {
        if *budget == 0 {
            return None;
        }
        *budget -= 1;
        if let Some(path) = self.attribute("AXURL").and_then(|v| v.file_path()) {
            return Some(path);
        }
        if depth == 0 {
            return None;
        }
        let children = self.attribute("AXChildren")?;
        unsafe {
            if CFGetTypeID(children.0) != CFArrayGetTypeID() {
                return None;
            }
            for i in 0..CFArrayGetCount(children.0.cast()).min(*budget as isize) {
                let child = CFArrayGetValueAtIndex(children.0.cast(), i);
                core_foundation::base::CFRetain(child);
                if let Some(path) = Value(child).item_path(depth - 1, budget) {
                    return Some(path);
                }
            }
        }
        None
    }
}

unsafe fn ns_string(text: &str) -> id {
    let value = NSString::alloc(nil).init_str(text);
    msg_send![value, autorelease]
}
unsafe fn string(value: id) -> Option<String> {
    if value == nil {
        return None;
    }
    let bytes = NSString::UTF8String(value);
    if bytes.is_null() {
        return None;
    }
    Some(CStr::from_ptr(bytes).to_string_lossy().into_owned())
}

pub fn drag_pasteboard(read_files: bool) -> Result<(i64, Vec<String>)> {
    unsafe {
        let pool = NSAutoreleasePool::new(nil);
        let result = (|| {
            let board: id = msg_send![class!(NSPasteboard), pasteboardWithName: NSDragPboard];
            if board == nil {
                bail!("无法读取系统拖拽会话");
            }
            let revision: i64 = msg_send![board, changeCount];
            if !read_files {
                return Ok((revision, vec![]));
            }
            let classes: id = msg_send![class!(NSArray), arrayWithObject: class!(NSURL)];
            let options: id = msg_send![class!(NSDictionary), dictionaryWithObject: {
                let yes: id = msg_send![class!(NSNumber), numberWithBool: true]; yes
            } forKey: NSPasteboardURLReadingFileURLsOnlyKey];
            let urls: id = msg_send![board, readObjectsForClasses: classes options: options];
            let mut paths = Vec::new();
            if urls != nil {
                if urls.count() > rshare_core::file_transfer::MAX_FILE_ENTRIES as u64 {
                    bail!("拖拽项目过多");
                }
                for index in 0..urls.count() {
                    let url = urls.objectAtIndex(index);
                    let local: bool = msg_send![url, isFileURL];
                    if !local {
                        continue;
                    }
                    let path: id = msg_send![url, path];
                    if let Some(path) = string(path) {
                        paths.push(path);
                    }
                }
            }
            Ok((revision, paths))
        })();
        pool.drain();
        result
    }
}

pub enum Destination {
    Path(PathBuf),
    Window([i32; 4]),
}

pub fn destination_folder(x: i32, y: i32) -> Result<Destination> {
    unsafe {
        let pool = NSAutoreleasePool::new(nil);
        let result = resolve(x, y);
        pool.drain();
        result
    }
}

unsafe fn resolve(x: i32, y: i32) -> Result<Destination> {
    let system = Value(AXUIElementCreateSystemWide());
    AXUIElementSetMessagingTimeout(system.0, 0.5);
    let mut hit = std::ptr::null();
    if AXUIElementCopyElementAtPosition(system.0, x as f32, y as f32, &mut hit) != 0
        || hit.is_null()
    {
        bail!("无法识别松手位置，请授予 R-ShareMouse 辅助功能权限");
    }
    let mut element = Value(hit);
    let mut pid = 0;
    if AXUIElementGetPid(element.0, &mut pid) != 0 {
        bail!("无法识别目标应用");
    }
    let app: id =
        msg_send![class!(NSRunningApplication), runningApplicationWithProcessIdentifier: pid];
    let bundle: id = msg_send![app, bundleIdentifier];
    if string(bundle).as_deref() != Some("com.apple.finder") {
        bail!("请松手到 Finder 文件夹窗口中");
    }
    let mut content = false;
    let mut item = false;
    for _ in 0..24 {
        if let Some(path) = element.attribute("AXURL").and_then(|v| v.file_path()) {
            if path.is_dir() {
                return Ok(Destination::Path(path));
            }
            bail!("不能投放到文件，请使用文件夹或文件列表空白处");
        }
        let role = element
            .attribute("AXRole")
            .and_then(|v| v.string())
            .unwrap_or_default();
        if matches!(role.as_str(), "AXCell" | "AXRow") {
            if let Some(path) = element.item_path(3, &mut 64) {
                if path.is_dir() {
                    return Ok(Destination::Path(path));
                }
                bail!("不能投放到文件，请使用文件夹或文件列表空白处");
            }
        }
        if role == "AXOutline"
            && element
                .attribute("AXIdentifier")
                .and_then(|v| v.string())
                .as_deref()
                != Some("ListView")
        {
            bail!("请松手到 Finder 文件列表中，不支持侧边栏投放");
        }
        if matches!(
            role.as_str(),
            "AXToolbar" | "AXTextField" | "AXButton" | "AXMenuBar"
        ) {
            bail!("请松手到 Finder 文件夹的图标区域或文件列表空白处");
        }
        // An item without a file URL cannot safely be interpreted as its parent folder.
        item |= matches!(role.as_str(), "AXCell" | "AXRow" | "AXImage");
        content |= matches!(
            role.as_str(),
            "AXScrollArea" | "AXList" | "AXBrowser" | "AXTable" | "AXOutline"
        );
        if role == "AXWindow" {
            if item {
                bail!("此 Finder 项目没有可识别的文件地址，请松手到文件列表空白处");
            }
            if !content {
                bail!("请松手到 Finder 文件列表空白处");
            }
            if let Some(folder) = element
                .attribute("AXDocument")
                .and_then(|v| v.file_path())
                .or_else(|| {
                    element
                        .attribute("AXProxy")
                        .and_then(|v| v.item_path(2, &mut 16))
                })
            {
                if !folder.is_dir() {
                    bail!("目标文件夹已不存在");
                }
                return Ok(Destination::Path(folder));
            }
            let position = element
                .attribute("AXPosition")
                .context("无法读取目标窗口位置")?;
            let size = element
                .attribute("AXSize")
                .context("无法读取目标窗口大小")?;
            let mut point = core_graphics::geometry::CGPoint::new(0.0, 0.0);
            let mut dimensions = core_graphics::geometry::CGSize::new(0.0, 0.0);
            if CFGetTypeID(position.0) != AXValueGetTypeID()
                || CFGetTypeID(size.0) != AXValueGetTypeID()
                || !AXValueGetValue(
                    position.0,
                    1,
                    (&mut point as *mut core_graphics::geometry::CGPoint).cast(),
                )
                || !AXValueGetValue(
                    size.0,
                    2,
                    (&mut dimensions as *mut core_graphics::geometry::CGSize).cast(),
                )
            {
                bail!("无法定位目标 Finder 窗口");
            }
            return Ok(Destination::Window([
                point.x.round() as i32,
                point.y.round() as i32,
                (point.x + dimensions.width).round() as i32,
                (point.y + dimensions.height).round() as i32,
            ]));
        }
        element = element
            .attribute("AXParent")
            .context("没有找到 Finder 文件夹窗口")?;
    }
    bail!("无法定位 Finder 文件夹")
}
