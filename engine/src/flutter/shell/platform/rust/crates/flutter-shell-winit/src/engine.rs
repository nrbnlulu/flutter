//! Statically selected access to the private C++ engine bridge.

use std::ffi::c_void;

use flutter_shell_core::FlutterRustTaskRunnerCallbacks;

pub(crate) trait EngineBridge {
    fn create_task_runner(callbacks: FlutterRustTaskRunnerCallbacks) -> *mut c_void;
    fn run_task(task_runner: *mut c_void, task_baton: u64) -> i32;
    fn destroy_task_runner(task_runner: *mut c_void);
}

#[cfg(not(test))]
pub(crate) struct FfiEngine;

#[cfg(not(test))]
impl EngineBridge for FfiEngine {
    fn create_task_runner(callbacks: FlutterRustTaskRunnerCallbacks) -> *mut c_void {
        unsafe extern "C" {
            fn FlutterRustShellCreateTaskRunner(
                callbacks: FlutterRustTaskRunnerCallbacks,
            ) -> *mut c_void;
        }
        unsafe { FlutterRustShellCreateTaskRunner(callbacks) }
    }

    fn run_task(task_runner: *mut c_void, task_baton: u64) -> i32 {
        unsafe extern "C" {
            fn FlutterRustShellRunTask(task_runner: *mut c_void, task_baton: u64) -> i32;
        }
        unsafe { FlutterRustShellRunTask(task_runner, task_baton) }
    }

    fn destroy_task_runner(task_runner: *mut c_void) {
        unsafe extern "C" {
            fn FlutterRustShellDestroyTaskRunner(task_runner: *mut c_void);
        }
        unsafe { FlutterRustShellDestroyTaskRunner(task_runner) }
    }
}

#[cfg(not(test))]
pub(crate) type CurrentEngine = FfiEngine;

#[cfg(test)]
pub(crate) type CurrentEngine = TestEngine;

#[cfg(test)]
pub(crate) struct TestEngine;

#[cfg(test)]
impl EngineBridge for TestEngine {
    fn create_task_runner(_: FlutterRustTaskRunnerCallbacks) -> *mut c_void {
        std::ptr::dangling_mut()
    }

    fn run_task(_: *mut c_void, _: u64) -> i32 {
        0
    }

    fn destroy_task_runner(_: *mut c_void) {}
}
