//! Task management implementation
//!
//! Everything about task management, like starting and switching tasks is
//! implemented here.
//!
//! A single global instance of [`TaskManager`] called `TASK_MANAGER` controls
//! all the tasks in the operating system.
//!
//! Be careful when you see [`__switch`]. Control flow around this function
//! might not be what you expect.

mod context;
mod switch;
#[allow(clippy::module_inception)]
mod task;

use crate::loader::{get_app_data, get_num_app};
use crate::sync::UPSafeCell;
use crate::trap::TrapContext;
use alloc::vec::Vec;
use lazy_static::*;
pub use switch::__switch;
pub use task::{TaskControlBlock, TaskStatus};
pub use crate::mm::*;
use crate::syscall::TaskInfo;
use crate::timer::{get_runtime, get_time_us};

pub use context::TaskContext;

/// The task manager, where all the tasks are managed.
///
/// Functions implemented on `TaskManager` deals with all task state transitions
/// and task context switching. For convenience, you can find wrappers around it
/// in the module level.
///
/// Most of `TaskManager` are hidden behind the field `inner`, to defer
/// borrowing checks to runtime. You can see examples on how to use `inner` in
/// existing functions on `TaskManager`.
pub struct TaskManager {
    /// total number of tasks
    num_app: usize,
    /// use inner value to get mutable access
    inner: UPSafeCell<TaskManagerInner>,
}

/// The task manager inner in 'UPSafeCell'
struct TaskManagerInner {
    /// task list
    tasks: Vec<TaskControlBlock>,
    /// id of current `Running` task
    current_task: usize,
}

lazy_static! {
    /// a `TaskManager` instance through lazy_static!
    pub static ref TASK_MANAGER: TaskManager = {
        info!("init TASK_MANAGER");
        let num_app = get_num_app();
        info!("num_app = {}", num_app);
        let mut tasks: Vec<TaskControlBlock> = Vec::new();
        for i in 0..num_app {
            tasks.push(TaskControlBlock::new(get_app_data(i), i));
        }
        TaskManager {
            num_app,
            inner: unsafe {
                UPSafeCell::new(TaskManagerInner {
                    tasks,
                    current_task: 0,
                })
            },
        }
    };
}

impl TaskManager {
    /// Run the first task in task list.
    ///
    /// Generally, the first task in task list is an idle task (we call it zero process later).
    /// But in ch4, we load apps statically, so the first task is a real app.
    fn run_first_task(&self) -> ! {
        let mut inner = self.inner.exclusive_access();
        let next_task = &mut inner.tasks[0];
        next_task.task_status = TaskStatus::Running;
        match next_task.start_time {
            None => next_task.start_time = Some(get_time_us()),
            _ => (),
        };
        let next_task_cx_ptr = &next_task.task_cx as *const TaskContext;
        drop(inner);
        let mut _unused = TaskContext::zero_init();
        // before this, we should drop local variables that must be dropped manually
        unsafe {
            __switch(&mut _unused as *mut _, next_task_cx_ptr);
        }
        panic!("unreachable in run_first_task!");
    }

    /// Change the status of current `Running` task into `Ready`.
    fn mark_current_suspended(&self) {
        let mut inner = self.inner.exclusive_access();
        let current = inner.current_task;
        inner.tasks[current].task_status = TaskStatus::Ready;
    }

    /// Change the status of current `Running` task into `Exited`.
    fn mark_current_exited(&self) {
        let mut inner = self.inner.exclusive_access();
        let current = inner.current_task;
        inner.tasks[current].task_status = TaskStatus::Exited;
    }

    /// Find next task to run and return task id.
    ///
    /// In this case, we only return the first `Ready` task in task list.
    fn find_next_task(&self) -> Option<usize> {
        let inner = self.inner.exclusive_access();
        let current = inner.current_task;
        (current + 1..current + self.num_app + 1)
            .map(|id| id % self.num_app)
            .find(|id| inner.tasks[*id].task_status == TaskStatus::Ready)
    }

    /// Get the current 'Running' task's token.
    fn get_current_token(&self) -> usize {
        let inner = self.inner.exclusive_access();
        inner.tasks[inner.current_task].get_user_token()
    }

    #[allow(clippy::mut_from_ref)]
    /// Get the current 'Running' task's trap contexts.
    fn get_current_trap_cx(&self) -> &mut TrapContext {
        let inner = self.inner.exclusive_access();
        inner.tasks[inner.current_task].get_trap_cx()
    }

    /// Switch current `Running` task to the task we have found,
    /// or there is no `Ready` task and we can exit with all applications completed
    fn run_next_task(&self) {
        if let Some(next) = self.find_next_task() {
            let mut inner = self.inner.exclusive_access();
            let current = inner.current_task;
            inner.tasks[next].task_status = TaskStatus::Running;
            inner.current_task = next;
            match inner.tasks[next].start_time {
                None => inner.tasks[next].start_time = Some(get_time_us()),
                _ => (),
            };
            let current_task_cx_ptr = &mut inner.tasks[current].task_cx as *mut TaskContext;
            let next_task_cx_ptr = &inner.tasks[next].task_cx as *const TaskContext;
            drop(inner);
            // before this, we should drop local variables that must be dropped manually
            unsafe {
                __switch(current_task_cx_ptr, next_task_cx_ptr);
            }
            // go back to user mode
        } else {
            panic!("All applications completed!");
        }
    }

    fn find_pte(&self, vpn: VirtPageNum) -> Option<PageTableEntry> {
        let inner = self.inner.exclusive_access();
        let memory_set = &inner.tasks[inner.current_task].memory_set;
        memory_set.find_pte(vpn)
    }

    fn unmap(&self, vpn_range: VPNRange) {
        let mut inner = self.inner.exclusive_access();
        let current_task = inner.current_task;
        let memory_set: &mut MemorySet = &mut (inner.tasks[current_task].memory_set);
        memory_set.unmap(vpn_range);
    }

    fn insert_framed_area(&self, start_va: VirtAddr,end_va: VirtAddr,map_perm: MapPermission) {
        let mut inner = self.inner.exclusive_access();
        let current_task = inner.current_task;
        let memory_set: &mut MemorySet = &mut (inner.tasks[current_task].memory_set);
        memory_set.insert_framed_area(start_va, end_va, map_perm);
    }

    fn update_syscall_time(&self, syscall_id: usize) {
        let mut inner = self.inner.exclusive_access();
        let current_task = inner.current_task;
        inner.tasks[current_task].syscall_times[syscall_id] += 1;
    }

    fn get_sys_task_info(&self, ti: *mut TaskInfo){
        let mut inner = self.inner.exclusive_access();
        let current_task = inner.current_task;
        unsafe {
            *ti = TaskInfo {
                status: TaskStatus::Running,
                syscall_times: inner.tasks[current_task].syscall_times.clone(),
                time: match inner.tasks[current_task].start_time {
                    Some(start_time) => get_runtime(start_time),
                    _ => 0,
                },
            }
        };

    }

    fn get_pa(&self, ptr: usize) -> usize{
        let va = VirtAddr::from(ptr);
        let inner = self.inner.exclusive_access();
        let memory_set = &inner.tasks[inner.current_task].memory_set;
        let pa: PhysAddr = memory_set.find_pte(va.floor()).unwrap().ppn().into();
        let pa: usize = pa.into();
        let result: usize = va.page_offset() + pa;
        result
    }
}

/// Run the first task in task list.
pub fn run_first_task() {
    TASK_MANAGER.run_first_task();
}

/// Switch current `Running` task to the task we have found,
/// or there is no `Ready` task and we can exit with all applications completed
fn run_next_task() {
    TASK_MANAGER.run_next_task();
}

/// Change the status of current `Running` task into `Ready`.
fn mark_current_suspended() {
    TASK_MANAGER.mark_current_suspended();
}

/// Change the status of current `Running` task into `Exited`.
fn mark_current_exited() {
    TASK_MANAGER.mark_current_exited();
}

/// Suspend the current 'Running' task and run the next task in task list.
pub fn suspend_current_and_run_next() {
    mark_current_suspended();
    run_next_task();
}

/// Exit the current 'Running' task and run the next task in task list.
pub fn exit_current_and_run_next() {
    mark_current_exited();
    run_next_task();
}

/// Get the current 'Running' task's token.
pub fn current_user_token() -> usize {
    TASK_MANAGER.get_current_token()
}

/// Get the current 'Running' task's trap contexts.
pub fn current_trap_cx() -> &'static mut TrapContext {
    TASK_MANAGER.get_current_trap_cx()
}



pub fn find_pte(vpn: VirtPageNum) -> Option<PageTableEntry> {
    TASK_MANAGER.find_pte(vpn)
}

pub fn unmap(vpn_range: VPNRange) {
    TASK_MANAGER.unmap(vpn_range);
}

pub fn insert_framed_area(start_va: VirtAddr,end_va: VirtAddr,map_perm: MapPermission) {
    TASK_MANAGER.insert_framed_area(start_va, end_va, map_perm);
}

pub fn update_syscall_time(syscall_id: usize) {
    TASK_MANAGER.update_syscall_time(syscall_id);
}

pub fn get_sys_task_info(ti: *mut TaskInfo){
    TASK_MANAGER.get_sys_task_info(ti);
}

pub fn get_pa(ptr: usize) -> usize{
    TASK_MANAGER.get_pa(ptr)
}