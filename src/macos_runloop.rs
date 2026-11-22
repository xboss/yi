//! Minimal CoreFoundation main RunLoop for a headless macOS process.
//! Translation.framework delivers completion callbacks through the main queue.

use std::ffi::c_void;

// The signatures below correspond to CoreFoundation/CFRunLoop.h and CFBase.h.
// CFOptionFlags = unsigned long and CFIndex = signed long on 64-bit macOS.
#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFAbsoluteTimeGetCurrent() -> f64;
    fn CFRunLoopGetCurrent() -> *mut c_void;
    fn CFRunLoopGetMain() -> *mut c_void;
    fn CFRunLoopRun();
    fn CFRunLoopStop(run_loop: *mut c_void);
    fn CFRunLoopWakeUp(run_loop: *mut c_void);

    fn CFRunLoopTimerCreate(
        allocator: *const c_void,
        fire_date: f64,
        interval: f64,
        flags: usize,
        order: isize,
        callback: extern "C" fn(*mut c_void, *mut c_void),
        context: *mut c_void,
    ) -> *mut c_void;
    fn CFRunLoopAddTimer(run_loop: *mut c_void, timer: *mut c_void, mode: *const c_void);
    fn CFRunLoopTimerInvalidate(timer: *mut c_void);
    fn CFRelease(object: *const c_void);
    static kCFRunLoopDefaultMode: *const c_void;
}

extern "C" fn no_op_timer(_: *mut c_void, _: *mut c_void) {}

pub fn run_main_loop() {
    // CFRunLoopRun() can exit immediately when it has no timers or sources.
    // This repeating timer keeps the RunLoop alive while the Axum server runs.
    unsafe {
        let timer = CFRunLoopTimerCreate(
            std::ptr::null(),
            CFAbsoluteTimeGetCurrent() + 60.0,
            60.0,
            0,
            0,
            no_op_timer,
            std::ptr::null_mut(),
        );
        assert!(!timer.is_null(), "CFRunLoopTimerCreate failed");
        let run_loop = CFRunLoopGetCurrent();
        CFRunLoopAddTimer(run_loop, timer, kCFRunLoopDefaultMode);
        CFRunLoopRun();
        CFRunLoopTimerInvalidate(timer);
        CFRelease(timer.cast_const());
    }
}

/// Called by the Axum thread after the HTTP server terminates.
pub fn request_stop() {
    unsafe {
        let main = CFRunLoopGetMain();
        if !main.is_null() {
            CFRunLoopStop(main);
            CFRunLoopWakeUp(main);
        }
    }
}
