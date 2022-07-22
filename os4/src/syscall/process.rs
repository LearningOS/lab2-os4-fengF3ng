//! Process management syscalls

use crate::config::MAX_SYSCALL_NUM;
use crate::task::{exit_current_and_run_next, suspend_current_and_run_next, TaskStatus, find_pte, unmap, insert_framed_area, get_sys_task_info, get_pa};
use crate::timer::get_time_us;
use crate::mm::{VirtAddr, MapPermission, VPNRange};

#[repr(C)]
#[derive(Debug)]
pub struct TimeVal {
    pub sec: usize,
    pub usec: usize,
}

#[derive(Clone, Copy)]
pub struct TaskInfo {
    pub status: TaskStatus,
    pub syscall_times: [u32; MAX_SYSCALL_NUM],
    pub time: usize,
}

pub fn sys_exit(exit_code: i32) -> ! {
    info!("[kernel] Application exited with code {}", exit_code);
    exit_current_and_run_next();
    panic!("Unreachable in sys_exit!");
}

/// current task gives up resources for other tasks
pub fn sys_yield() -> isize {
    suspend_current_and_run_next();
    0
}

// YOUR JOB: 引入虚地址后重写 sys_get_time
pub fn sys_get_time(_ts: *mut TimeVal, _tz: usize) -> isize {
    let _us = get_time_us();
    let _ts = get_pa(_ts as usize) as *mut TimeVal;
     unsafe {
         *_ts = TimeVal {
             sec: _us / 1_000_000,
             usec: _us % 1_000_000,
         };
     }
    0
}

// CLUE: 从 ch4 开始不再对调度算法进行测试~
pub fn sys_set_priority(_prio: isize) -> isize {
    -1
}
/*
申请长度为 len 字节的物理内存（不要求实际物理内存位置，可以随便找一块），将其映射到 start 开始的虚存，内存页属性为 port

参数：
        start 需要映射的虚存起始地址，要求按页对齐
        len 映射字节长度，可以为 0
        port：第 0 位表示是否可读，第 1 位表示是否可写，第 2 位表示是否可执行。其他位无效且必须为 0
返回值：执行成功则返回 0，错误返回 -1
说明：
        为了简单，目标虚存区间要求按页对齐，len 可直接按页向上取整，不考虑分配失败时的页回收。
可能的错误：
        start 没有按页大小对齐
        port & !0x7 != 0 (port 其余位必须为0)
        port & 0x7 = 0 (这样的内存无意义)
        [start, start + len) 中存在已经被映射的页
        物理内存不足
*/
// YOUR JOB: 扩展内核以实现 sys_mmap 和 sys_munmap
pub fn sys_mmap(_start: usize, _len: usize, _port: usize) -> isize {
    let start_va = VirtAddr::from(_start);
    let end_va = VirtAddr::from(_start+_len);
    // check valid
    if !start_va.aligned() {
        println!("va aligned fail!");
        return -1;
    }
    if (_port & !0x7 != 0) || (_port & 0x7 == 0) {
        println!("port invalid");
        return -1;
    }
    let vpn_range = VPNRange::new(start_va.floor(), end_va.ceil());
    // check if mapped
    for vpn in vpn_range {
        if let Some(_) = find_pte(vpn) {
            println!("already exist mapped page!");
            return -1;
        }
    }
    // map
    let mut map_perm = MapPermission::U;
    map_perm |= MapPermission::from_bits((_port as u8) << 1).unwrap();
    insert_framed_area(
        start_va,
        end_va,
        map_perm
    );
    // check if success
    for vpn in vpn_range {
        match find_pte(vpn) {
            None => {
                println!("sys_mmap fail!");
                return -1;
            },
            _ => (),
        }
    }
    0
}

/*
取消到 [start, start + len) 虚存的映射

参数和返回值请参考 mmap

说明：
        为了简单，参数错误时不考虑内存的恢复和回收。
可能的错误：
        [start, start + len) 中存在未被映射的虚存。
*/
pub fn sys_munmap(_start: usize, _len: usize) -> isize {
    let start_va = VirtAddr::from(_start);
    let end_va = VirtAddr::from(_start+_len);
    // check valid
    if !start_va.aligned() {
        println!("va aligned fail!");
        return -1;
    }
    let vpn_range = VPNRange::new(start_va.floor(), end_va.ceil());
    // check unmapped
    for vpn in vpn_range {
        match find_pte(vpn) {
            None => {
                println!("exist unmapped page!");
                return -1;
            },
            _ => (),
        };
    }
    // unmap
    unmap(vpn_range);
    0
}

// YOUR JOB: 引入虚地址后重写 sys_task_info
pub fn sys_task_info(ti: *mut TaskInfo) -> isize {
    let ti = get_pa(ti as usize) as *mut TaskInfo;
    get_sys_task_info(ti);
    0
}
