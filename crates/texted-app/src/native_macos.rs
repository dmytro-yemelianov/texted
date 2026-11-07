//! Adds the two document lifecycle callbacks absent from winit 0.29's application delegate.
//! Existing delegate methods are never replaced; all editor work is queued into the Rust event loop.
use crate::{documents::Action, AppEvent};
use winit::event_loop::EventLoopProxy;
#[cfg(target_os = "macos")]
mod mac {
    use super::*;
    use objc2::{
        class, msg_send,
        runtime::{AnyClass, AnyObject, Sel},
        sel,
    };
    use std::{
        cell::OnceCell,
        ffi::{c_char, c_void, CStr},
        path::PathBuf,
    };
    thread_local! {static PROXY:OnceCell<EventLoopProxy<AppEvent>>=const {OnceCell::new()};}
    #[link(name = "objc")]
    extern "C" {
        fn class_addMethod(
            class: *const AnyClass,
            name: Sel,
            implementation: *const c_void,
            types: *const c_char,
        ) -> bool;
    }
    unsafe extern "C" fn open_files(
        _this: *mut AnyObject,
        _sel: Sel,
        application: *mut AnyObject,
        files: *mut AnyObject,
    ) {
        // AppKit supplies NSArray<NSString *>; open every path in its own tab.
        let count: usize = unsafe { msg_send![files, count] };
        for index in 0..count {
            let path: *mut AnyObject = unsafe { msg_send![files,objectAtIndex:index] };
            let utf8: *const c_char = unsafe { msg_send![path, UTF8String] };
            if !utf8.is_null() {
                let path = PathBuf::from(
                    unsafe { CStr::from_ptr(utf8) }
                        .to_string_lossy()
                        .into_owned(),
                );
                PROXY.with(|p| {
                    if let Some(proxy) = p.get() {
                        let _ = proxy.send_event(AppEvent::Document(Action::OpenPath(path)));
                    }
                });
            }
        }
        unsafe {
            let _: () = msg_send![application,replyToOpenOrPrint:0usize];
        }
    }
    unsafe extern "C" fn should_terminate(
        _this: *mut AnyObject,
        _sel: Sel,
        _application: *mut AnyObject,
    ) -> usize {
        PROXY.with(|p| {
            if let Some(proxy) = p.get() {
                let _ = proxy.send_event(AppEvent::Quit);
            }
        });
        0 // NSTerminateCancel: Rust's guarded close will exit the event loop after the user decides.
    }
    pub fn install(proxy: EventLoopProxy<AppEvent>) -> Result<(), String> {
        PROXY
            .with(|p| p.set(proxy))
            .map_err(|_| "Native file callbacks already installed".to_owned())?;
        // Called on main after EventLoop construction, while winit owns the delegate for the process lifetime.
        unsafe {
            let application: *mut AnyObject = msg_send![class!(NSApplication), sharedApplication];
            let delegate: *mut AnyObject = msg_send![application, delegate];
            if delegate.is_null() {
                return Err("NSApplication has no delegate".into());
            }
            let class = (&*delegate).class();
            let opened = class_addMethod(
                class,
                sel!(application:openFiles:),
                open_files as *const c_void,
                c"v@:@@".as_ptr(),
            );
            let quitting = class_addMethod(
                class,
                sel!(applicationShouldTerminate:),
                should_terminate as *const c_void,
                c"Q@:@".as_ptr(),
            );
            if !opened || !quitting {
                return Err("The native delegate already implements document callbacks".into());
            }
        }
        Ok(())
    }
}
#[cfg(target_os = "macos")]
pub use mac::install;
#[cfg(not(target_os = "macos"))]
pub fn install(_proxy: EventLoopProxy<AppEvent>) -> Result<(), String> {
    Ok(())
}
