//! Adds the two document lifecycle callbacks absent from winit 0.29's application delegate.
//! Existing delegate methods are never replaced; all editor work is queued into the Rust event loop.
use crate::{documents::Action, AppEvent};
use winit::event_loop::EventLoopProxy;
#[cfg(target_os = "macos")]
mod mac {
    use super::*;
    use objc2::{
        class, msg_send,
        runtime::{AnyClass, AnyObject, Bool, Sel},
        sel,
    };
    use std::{
        cell::{Cell, OnceCell},
        ffi::{c_char, c_void, CStr},
        path::PathBuf,
    };
    thread_local! {
        static PROXY: OnceCell<EventLoopProxy<AppEvent>> = const { OnceCell::new() };
        // AppKit waits for one reply per NSTerminateLater; the close sequence decides it.
        static TERMINATE_PENDING: Cell<bool> = const { Cell::new(false) };
    }
    const NS_TERMINATE_NOW: usize = 1;
    const NS_TERMINATE_LATER: usize = 2;
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
        let queued = PROXY.with(|p| {
            p.get()
                .is_some_and(|proxy| proxy.send_event(AppEvent::Quit).is_ok())
        });
        if !queued {
            return NS_TERMINATE_NOW; // The event loop is already gone; nothing is left to save.
        }
        // Logout and restart wait for this answer. Cancel would abort them even with
        // nothing to save, so defer it until the guarded close sequence finishes.
        TERMINATE_PENDING.set(true);
        NS_TERMINATE_LATER
    }
    fn reply_to_terminate(accepted: bool) {
        unsafe {
            let application: *mut AnyObject = msg_send![class!(NSApplication), sharedApplication];
            let _: () =
                msg_send![application, replyToApplicationShouldTerminate: Bool::new(accepted)];
        }
    }
    unsafe extern "C" fn accept_later(_this: *mut AnyObject, _sel: Sel, _argument: *mut AnyObject) {
        reply_to_terminate(true);
    }
    pub fn terminate_pending() -> bool {
        TERMINATE_PENDING.get()
    }
    /// Declines a deferred system quit request after the close sequence was cancelled.
    pub fn cancel_terminate() {
        if TERMINATE_PENDING.replace(false) {
            reply_to_terminate(false);
        }
    }
    /// Lets a deferred system quit proceed; AppKit then ends the process without unwinding
    /// Rust frames, so drop the app first. Accepting inside the winit callback would re-enter
    /// it from applicationWillTerminate:, so the reply runs on the next run-loop pass.
    pub fn accept_terminate() {
        if !TERMINATE_PENDING.replace(false) {
            return;
        }
        unsafe {
            let application: *mut AnyObject = msg_send![class!(NSApplication), sharedApplication];
            let delegate: *mut AnyObject = msg_send![application, delegate];
            let _: () = msg_send![
                delegate,
                performSelectorOnMainThread: sel!(textedAcceptTerminate:),
                withObject: std::ptr::null_mut::<AnyObject>(),
                waitUntilDone: Bool::NO
            ];
        }
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
            let accepting = class_addMethod(
                class,
                sel!(textedAcceptTerminate:),
                accept_later as *const c_void,
                c"v@:@".as_ptr(),
            );
            if !opened || !quitting || !accepting {
                return Err("The native delegate already implements document callbacks".into());
            }
        }
        Ok(())
    }
}
#[cfg(target_os = "macos")]
pub use mac::{accept_terminate, cancel_terminate, install, terminate_pending};
#[cfg(not(target_os = "macos"))]
pub fn install(_proxy: EventLoopProxy<AppEvent>) -> Result<(), String> {
    Ok(())
}
#[cfg(not(target_os = "macos"))]
pub fn cancel_terminate() {}
#[cfg(not(target_os = "macos"))]
pub fn accept_terminate() {}
#[cfg(not(target_os = "macos"))]
pub fn terminate_pending() -> bool {
    false
}
