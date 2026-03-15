use anyhow::{Context, Result, bail};
use isa::{
    CommitRecord, DecodedInstruction, EngineKind, RunMetrics, RunResult, StageTraceEvent,
    TRAP_ILLEGAL_INST, decode_word,
};
use libc::{clock_gettime, getpid, timespec};
use runtime::{GuestMemory, GuestRuntime, MEM_READ, MEM_WRITE, guest_prot_to_region_flags};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::ffi::CString;
use std::path::PathBuf;

const REG_ZERO: usize = 0;
const REG_SP: usize = 1;
const REG_A0: usize = 2;
const REG_A1: usize = 3;
const REG_A2: usize = 4;
const REG_A3: usize = 5;
const REG_A4: usize = 6;
const REG_A5: usize = 7;
const REG_A7: usize = 9;
const REG_RA: usize = 10;
const REG_T1: usize = 24;
const REG_T4: usize = 27;
const REG_U1: usize = 28;
const REG_U3: usize = 30;
const REG_U4: usize = 31;
const REG_IMPLICIT_T_DST: usize = REG_U4;
const REG_IMPLICIT_U_DST: usize = REG_U3;

const SYS_EVENTFD2: u64 = 19;
const SYS_EPOLL_CREATE1: u64 = 20;
const SYS_EPOLL_CTL: u64 = 21;
const SYS_EPOLL_PWAIT: u64 = 22;
const SYS_GETCWD: u64 = 17;
const SYS_DUP3: u64 = 24;
const SYS_FCNTL: u64 = 25;
const SYS_IOCTL: u64 = 29;
const SYS_READ: u64 = 63;
const SYS_WRITE: u64 = 64;
const SYS_OPENAT: u64 = 56;
const SYS_CLOSE: u64 = 57;
const SYS_PIPE2: u64 = 59;
const SYS_LSEEK: u64 = 62;
const SYS_PSELECT6: u64 = 72;
const SYS_PPOLL: u64 = 73;
const SYS_READLINKAT: u64 = 78;
const SYS_NEWFSTATAT: u64 = 79;
const SYS_FSTAT: u64 = 80;
const SYS_FUTEX: u64 = 98;
const SYS_SET_TID_ADDRESS: u64 = 96;
const SYS_SET_ROBUST_LIST: u64 = 99;
const SYS_SETGID: u64 = 144;
const SYS_SETUID: u64 = 146;
const SYS_SETRESUID: u64 = 147;
const SYS_GETRESUID: u64 = 148;
const SYS_SETRESGID: u64 = 149;
const SYS_GETRESGID: u64 = 150;
const SYS_UNAME: u64 = 160;
const SYS_GETPPID: u64 = 173;
const SYS_BRK: u64 = 214;
const SYS_MUNMAP: u64 = 215;
const SYS_MMAP: u64 = 222;
const SYS_WAIT4: u64 = 260;
const SYS_MPROTECT: u64 = 226;
const SYS_MADVISE: u64 = 233;
const SYS_PRLIMIT64: u64 = 261;
const SYS_MEMBARRIER: u64 = 283;
const SYS_RSEQ: u64 = 293;
const SYS_SIGALTSTACK: u64 = 132;
const SYS_RT_SIGACTION: u64 = 134;
const SYS_RT_SIGPROCMASK: u64 = 135;
const SYS_CLOCK_GETTIME: u64 = 113;
const SYS_GETPID: u64 = 172;
const SYS_PRCTL: u64 = 167;
const SYS_GETUID: u64 = 174;
const SYS_GETEUID: u64 = 175;
const SYS_GETGID: u64 = 176;
const SYS_GETEGID: u64 = 177;
const SYS_GETTID: u64 = 178;
const SYS_SYSINFO: u64 = 179;
const SYS_GETRANDOM: u64 = 278;
const SYS_EXIT: u64 = 93;
const SYS_EXIT_GROUP: u64 = 94;

const TRAP_SW_BREAKPOINT: u64 = 50;
const PAGE_SIZE: u64 = 4096;
const MAX_C_STRING: usize = 4096;
const GUEST_AT_FDCWD: i32 = -100;
const GUEST_AT_EMPTY_PATH: i32 = 0x1000;
const GUEST_AT_SYMLINK_NOFOLLOW: i32 = 0x100;
const GUEST_F_GETFD: i32 = 1;
const GUEST_F_SETFD: i32 = 2;
const GUEST_F_GETFL: i32 = 3;
const GUEST_F_SETFL: i32 = 4;
const GUEST_F_DUPFD: i32 = 0;
const GUEST_F_DUPFD_CLOEXEC: i32 = 1030;
const GUEST_FD_CLOEXEC: i32 = 1;
const GUEST_EFD_SEMAPHORE: i32 = 1;
const GUEST_O_RDONLY: i32 = 0;
const GUEST_O_WRONLY: i32 = 1;
const GUEST_O_NONBLOCK: i32 = 0o4000;
const GUEST_O_CLOEXEC: i32 = 0o2000000;
const GUEST_EPOLL_CTL_ADD: i32 = 1;
const GUEST_EPOLL_CTL_DEL: i32 = 2;
const GUEST_EPOLL_CTL_MOD: i32 = 3;
const GUEST_EPOLLIN: u32 = 0x001;
const GUEST_EPOLLPRI: u32 = 0x002;
const GUEST_EPOLLOUT: u32 = 0x004;
const GUEST_EPOLLERR: u32 = 0x008;
const GUEST_EPOLLHUP: u32 = 0x010;
const GUEST_EPOLLNVAL: u32 = 0x020;
const GUEST_EPOLLRDNORM: u32 = 0x040;
const GUEST_EPOLLRDBAND: u32 = 0x080;
const GUEST_EPOLLWRNORM: u32 = 0x100;
const GUEST_EPOLLWRBAND: u32 = 0x200;
const GUEST_EPOLLRDHUP: u32 = 0x2000;
const GUEST_EPOLLET: u32 = 1 << 31;
const GUEST_EPOLLONESHOT: u32 = 1 << 30;
const GUEST_EPOLL_EVENT_SIZE: u64 = 16;
const GUEST_TIOCGPGRP: u64 = 0x540F;
const GUEST_TIOCSPGRP: u64 = 0x5410;
const GUEST_TIOCGWINSZ: u64 = 0x5413;
const GUEST_POLLNVAL: u16 = 0x020;
const GUEST_SIGALTSTACK_SIZE: u64 = 24;
const GUEST_POLLFD_SIZE: u64 = 8;
const GUEST_FD_SET_SIZE: usize = 128;
const GUEST_SS_ONSTACK: u32 = 1;
const GUEST_SS_DISABLE: u32 = 2;
const GUEST_MINSIGSTKSZ: u64 = 2048;
const GUEST_SIGSET_MAX_BYTES: usize = 128;
const GUEST_PR_SET_NAME: u64 = 15;
const GUEST_PR_GET_NAME: u64 = 16;
const GUEST_PRCTL_NAME_BYTES: usize = 16;
const GUEST_MEMBARRIER_CMD_QUERY: u64 = 0;
const GUEST_MEMBARRIER_CMD_PRIVATE_EXPEDITED: u64 = 8;
const GUEST_MEMBARRIER_CMD_REGISTER_PRIVATE_EXPEDITED: u64 = 16;
const GUEST_RSEQ_FLAG_UNREGISTER: u64 = 1;
const GUEST_RSEQ_SIG: u32 = 0x5305_3053;
const GUEST_RSEQ_MIN_LEN: u32 = 32;
const GUEST_LINUX_STAT_SIZE: usize = 128;
const GUEST_SIGSET_BYTES: usize = 16;
const GUEST_UTS_FIELD_BYTES: usize = 65;
const GUEST_UTSNAME_SIZE: usize = GUEST_UTS_FIELD_BYTES * 6;
const GUEST_SYSINFO_SIZE: usize = 368;
const GUEST_RUSAGE_SIZE: usize = 272;
const FUTEX_WAIT: i32 = 0;
const FUTEX_WAKE: i32 = 1;
const FUTEX_PRIVATE: i32 = 128;
const FUTEX_CLOCK_REALTIME: i32 = 256;
const GUEST_EPERM: i32 = 1;
const GUEST_ENOENT: i32 = 2;
const GUEST_EEXIST: i32 = 17;
const GUEST_ECHILD: i32 = 10;
const GUEST_EAGAIN: i32 = 11;
const GUEST_EBADF: i32 = 9;
const GUEST_EFAULT: i32 = 14;
const GUEST_EINVAL: i32 = 22;
const GUEST_ENOTTY: i32 = 25;
const GUEST_ENOMEM: i32 = 12;
const GUEST_ENOSYS: i32 = 38;
const GUEST_ERANGE: i32 = 34;
const GUEST_ETIMEDOUT: i32 = 110;
const GUEST_ESRCH: i32 = 3;
const RLIMIT_DATA: u32 = 2;
const RLIMIT_STACK: u32 = 3;
const RLIMIT_NOFILE: u32 = 7;
const RLIMIT_AS: u32 = 9;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FuncRunOptions {
    pub max_steps: u64,
}

impl Default for FuncRunOptions {
    fn default() -> Self {
        Self { max_steps: 100_000 }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FuncRunBundle {
    pub result: RunResult,
    pub stage_events: Vec<StageTraceEvent>,
}

#[derive(Debug, Default)]
pub struct FuncEngine;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExitSignal {
    GuestExit(i32),
    Breakpoint,
    Fault,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BlockKind {
    Fall,
    Direct,
    Cond,
    Call,
    Ind,
    ICall,
    Ret,
}

#[derive(Debug, Clone)]
struct BlockContext {
    kind: BlockKind,
    target: Option<u64>,
    return_target: Option<u64>,
}

#[derive(Debug, Clone)]
enum SpecialFdKind {
    EventFd(EventFdState),
    Epoll(EpollState),
}

#[derive(Debug, Clone)]
struct EventFdState {
    write_fd: i32,
    counter: u64,
    semaphore: bool,
}

#[derive(Debug, Clone)]
struct GuestEpollRegistration {
    guest_fd: u64,
    events: u32,
    data: u64,
}

#[derive(Debug, Clone)]
struct EpollState {
    wake_write_fd: i32,
    registrations: BTreeMap<u64, GuestEpollRegistration>,
}

#[derive(Debug, Clone)]
struct ExecState {
    pc: u64,
    regs: [u64; 32],
    memory: GuestMemory,
    ssr: BTreeMap<u16, u64>,
    fd_table: HashMap<u64, i32>,
    fd_status_flags: HashMap<u64, i32>,
    fd_fd_flags: HashMap<u64, i32>,
    special_fds: HashMap<u64, SpecialFdKind>,
    block: Option<BlockContext>,
    cond: bool,
    carg: bool,
    target: u64,
    brk_base: u64,
    brk_current: u64,
    mmap_cursor: u64,
    clear_child_tid: u64,
    robust_list_head: u64,
    robust_list_len: u64,
    current_pid: u64,
    current_ppid: u64,
    current_pgrp: u32,
    uid: u32,
    euid: u32,
    suid: u32,
    gid: u32,
    egid: u32,
    sgid: u32,
    random_state: u64,
    thread_name: [u8; GUEST_PRCTL_NAME_BYTES],
    membarrier_private_expedited: bool,
    rseq_addr: u64,
    rseq_len: u32,
    rseq_sig: u32,
    alt_stack_sp: u64,
    alt_stack_size: u64,
    alt_stack_flags: u32,
    rlimits: BTreeMap<u32, GuestRlimit>,
}

#[derive(Debug, Clone)]
struct StepOutcome {
    next_pc: u64,
    exit: Option<ExitSignal>,
    retire_cause: String,
}

#[derive(Debug, Clone, Copy)]
struct GuestLinuxStat {
    dev: u64,
    ino: u64,
    mode: u32,
    nlink: u32,
    uid: u32,
    gid: u32,
    rdev: u64,
    size: i64,
    blksize: i32,
    blocks: i64,
    atime_sec: i64,
    atime_nsec: u64,
    mtime_sec: i64,
    mtime_nsec: u64,
    ctime_sec: i64,
    ctime_nsec: u64,
}

#[derive(Debug, Clone, Copy)]
struct GuestRlimit {
    cur: u64,
    max: u64,
}

impl FuncEngine {
    pub fn run(&self, runtime: &GuestRuntime, options: &FuncRunOptions) -> Result<FuncRunBundle> {
        let mut state = ExecState::from_runtime(runtime);
        let mut commits = Vec::new();
        let mut decoded = Vec::<DecodedInstruction>::new();
        let mut stage_events = Vec::<StageTraceEvent>::new();
        let mut exit_reason = "step_limit".to_string();
        let mut step = 0u64;

        while step < options.max_steps {
            state.ssr.insert(0x0C00, step);
            let pc = state.pc;
            let bundle = state
                .memory
                .fetch_u64_bundle(pc)
                .with_context(|| format!("no mapped instruction bundle at pc=0x{pc:016x}"))?;

            let Some(decoded_insn) = decode_word(bundle) else {
                let mut commit =
                    CommitRecord::unsupported(step, pc, bundle, TRAP_ILLEGAL_INST, &runtime.block);
                commit.len = 8;
                commit.next_pc = pc;
                commits.push(commit);
                stage_events.push(stage_event(step, runtime, "D1", "decode_miss"));
                exit_reason = "decode_fault".to_string();
                break;
            };

            if state.block.is_some() && starts_new_block(&decoded_insn) {
                let next_pc = resolve_block_end(&mut state, pc);
                if next_pc != pc {
                    state.pc = next_pc;
                    continue;
                }
            }

            let mut commit = empty_commit(step, pc, &decoded_insn, runtime);
            let outcome = match execute_step(&mut state, runtime, &decoded_insn, &mut commit) {
                Ok(outcome) => outcome,
                Err(_) => {
                    commit.trap_valid = 1;
                    commit.trap_cause = TRAP_ILLEGAL_INST;
                    commit.traparg0 = decoded_insn.instruction_bits;
                    commit.next_pc = pc;
                    StepOutcome {
                        next_pc: pc,
                        exit: Some(ExitSignal::Fault),
                        retire_cause: format!("unsupported:{}", decoded_insn.mnemonic),
                    }
                }
            };

            decoded.push(decoded_insn.clone());
            stage_events.push(stage_event(step * 4, runtime, "F0", "fetch"));
            stage_events.push(stage_event(
                step * 4 + 1,
                runtime,
                "D1",
                &format!("decode:{}", decoded_insn.mnemonic),
            ));
            stage_events.push(stage_event(
                step * 4 + 2,
                runtime,
                "E1",
                &outcome.retire_cause,
            ));
            stage_events.push(stage_event(step * 4 + 3, runtime, "CMT", "retire"));

            state.pc = outcome.next_pc;
            commits.push(commit);
            step += 1;

            if let Some(exit) = outcome.exit {
                exit_reason = match exit {
                    ExitSignal::GuestExit(code) => format!("guest_exit({code})"),
                    ExitSignal::Breakpoint => "breakpoint".to_string(),
                    ExitSignal::Fault => "unsupported_instruction".to_string(),
                };
                break;
            }
        }

        let cycles = (commits.len() as u64).saturating_mul(4);
        let result = RunResult {
            image_name: runtime.image.image_name(),
            entry_pc: runtime.state.pc,
            metrics: RunMetrics {
                engine: EngineKind::Func,
                cycles,
                commits: commits.len() as u64,
                exit_reason,
            },
            commits,
            decoded,
        };
        Ok(FuncRunBundle {
            result,
            stage_events,
        })
    }
}

fn starts_new_block(decoded: &DecodedInstruction) -> bool {
    matches!(
        decoded.mnemonic.as_str(),
        "BSTART.STD"
            | "BSTART CALL"
            | "C.BSTART"
            | "C.BSTART.STD"
            | "HL.BSTART.STD"
            | "HL.BSTART.CALL"
            | "HL.BSTART.FP"
            | "HL.BSTART.SYS"
    )
}

impl ExecState {
    fn from_runtime(runtime: &GuestRuntime) -> Self {
        let mut ssr = BTreeMap::new();
        ssr.insert(0x0000, 0);
        ssr.insert(0x0001, 0);
        ssr.insert(0x0C00, 0);
        let current_pid = unsafe { getpid() }.max(0) as u64;
        let current_ppid = unsafe { libc::getppid() }.max(0) as u64;
        let default_nofile = GuestRlimit {
            cur: 1024,
            max: 4096,
        };
        let default_stack = GuestRlimit {
            cur: runtime.config.stack_size,
            max: runtime.config.stack_size,
        };
        let default_data = GuestRlimit {
            cur: runtime.config.mem_bytes,
            max: runtime.config.mem_bytes,
        };
        let default_as = GuestRlimit {
            cur: runtime.config.mem_bytes,
            max: runtime.config.mem_bytes,
        };

        let heap_base = align_up(runtime.memory.highest_mapped_address(), PAGE_SIZE);
        let mmap_cursor = heap_base.saturating_add(PAGE_SIZE);

        Self {
            pc: runtime.state.pc,
            regs: runtime.state.regs,
            memory: runtime.memory.clone(),
            ssr,
            fd_table: runtime.fd_table.clone(),
            fd_status_flags: HashMap::from([(0, 0), (1, 0), (2, 0)]),
            fd_fd_flags: HashMap::from([(0, 0), (1, 0), (2, 0)]),
            special_fds: HashMap::new(),
            block: None,
            cond: false,
            carg: false,
            target: 0,
            brk_base: heap_base,
            brk_current: heap_base,
            mmap_cursor,
            clear_child_tid: 0,
            robust_list_head: 0,
            robust_list_len: 0,
            current_pid,
            current_ppid,
            current_pgrp: current_pid.min(u32::MAX as u64) as u32,
            uid: 0,
            euid: 0,
            suid: 0,
            gid: 0,
            egid: 0,
            sgid: 0,
            random_state: 0x4c69_6e78_434f_5245u64
                ^ runtime.state.pc
                ^ runtime.config.mem_bytes
                ^ runtime.config.stack_size,
            thread_name: {
                let mut name = [0u8; GUEST_PRCTL_NAME_BYTES];
                name[..4].copy_from_slice(b"lx-f");
                name
            },
            membarrier_private_expedited: false,
            rseq_addr: 0,
            rseq_len: 0,
            rseq_sig: 0,
            alt_stack_sp: 0,
            alt_stack_size: 0,
            alt_stack_flags: GUEST_SS_DISABLE,
            rlimits: BTreeMap::from([
                (RLIMIT_DATA, default_data),
                (RLIMIT_STACK, default_stack),
                (RLIMIT_NOFILE, default_nofile),
                (RLIMIT_AS, default_as),
            ]),
        }
    }

    fn read_reg(&self, reg: usize) -> u64 {
        self.regs.get(reg).copied().unwrap_or(0)
    }

    fn write_reg(&mut self, reg: usize, value: u64) {
        if reg != REG_ZERO {
            self.regs[reg] = value;
        }
        self.regs[REG_ZERO] = 0;
    }

    fn alloc_guest_fd(&self) -> u64 {
        self.alloc_guest_fd_from(3)
    }

    fn alloc_guest_fd_from(&self, min_fd: u64) -> u64 {
        let mut fd = min_fd.max(3);
        while self.fd_table.contains_key(&fd) {
            fd += 1;
        }
        fd
    }

    fn insert_guest_fd(&mut self, guest_fd: u64, host_fd: i32, status_flags: i32, fd_flags: i32) {
        self.release_guest_fd(guest_fd);
        self.fd_table.insert(guest_fd, host_fd);
        self.fd_status_flags.insert(guest_fd, status_flags);
        self.fd_fd_flags
            .insert(guest_fd, fd_flags & GUEST_FD_CLOEXEC);
    }

    fn duplicate_guest_fd(
        &mut self,
        guest_fd: u64,
        min_fd: u64,
        cloexec: bool,
    ) -> std::result::Result<u64, i32> {
        if self.special_fds.contains_key(&guest_fd) {
            return Err(GUEST_EINVAL);
        }
        let host_fd = self.host_fd(guest_fd)?;
        let duplicated = unsafe { libc::dup(host_fd) };
        if duplicated < 0 {
            return Err(last_errno());
        }
        let new_guest_fd = self.alloc_guest_fd_from(min_fd);
        let status_flags = self.fd_status_flags.get(&guest_fd).copied().unwrap_or(0);
        let fd_flags = if cloexec { GUEST_FD_CLOEXEC } else { 0 };
        self.insert_guest_fd(new_guest_fd, duplicated, status_flags, fd_flags);
        Ok(new_guest_fd)
    }

    fn duplicate_guest_fd_to(
        &mut self,
        guest_fd: u64,
        target_guest_fd: u64,
        cloexec: bool,
    ) -> std::result::Result<u64, i32> {
        if guest_fd == target_guest_fd {
            return Err(GUEST_EINVAL);
        }
        if self.special_fds.contains_key(&guest_fd) {
            return Err(GUEST_EINVAL);
        }
        let host_fd = self.host_fd(guest_fd)?;
        let duplicated = unsafe { libc::dup(host_fd) };
        if duplicated < 0 {
            return Err(last_errno());
        }
        let status_flags = self.fd_status_flags.get(&guest_fd).copied().unwrap_or(0);
        let fd_flags = if cloexec { GUEST_FD_CLOEXEC } else { 0 };
        self.insert_guest_fd(target_guest_fd, duplicated, status_flags, fd_flags);
        Ok(target_guest_fd)
    }

    fn push_t(&mut self, value: u64) {
        for reg in (REG_T1 + 1..=REG_T4).rev() {
            self.regs[reg] = self.regs[reg - 1];
        }
        self.regs[REG_T1] = value;
    }

    fn push_u(&mut self, value: u64) {
        for reg in (REG_U1 + 1..=REG_U4).rev() {
            self.regs[reg] = self.regs[reg - 1];
        }
        self.regs[REG_U1] = value;
    }

    fn host_fd(&self, guest_fd: u64) -> std::result::Result<i32, i32> {
        self.fd_table.get(&guest_fd).copied().ok_or(GUEST_EBADF)
    }

    fn release_guest_fd(&mut self, guest_fd: u64) {
        self.unregister_from_epolls(guest_fd);
        let host_fd = self.fd_table.remove(&guest_fd);
        self.fd_status_flags.remove(&guest_fd);
        self.fd_fd_flags.remove(&guest_fd);
        match self.special_fds.remove(&guest_fd) {
            Some(SpecialFdKind::EventFd(eventfd)) => {
                if let Some(read_fd) = host_fd {
                    close_host_fd(read_fd);
                }
                close_host_fd(eventfd.write_fd);
            }
            Some(SpecialFdKind::Epoll(epoll)) => {
                if let Some(read_fd) = host_fd {
                    close_host_fd(read_fd);
                }
                close_host_fd(epoll.wake_write_fd);
            }
            None => {
                if let Some(fd) = host_fd {
                    close_host_fd(fd);
                }
            }
        }
    }

    fn close_guest_fd(&mut self, guest_fd: u64) -> std::result::Result<u64, i32> {
        if !self.fd_table.contains_key(&guest_fd) {
            return Err(GUEST_EBADF);
        }
        self.release_guest_fd(guest_fd);
        Ok(0)
    }

    fn unregister_from_epolls(&mut self, guest_fd: u64) {
        for special in self.special_fds.values_mut() {
            if let SpecialFdKind::Epoll(epoll) = special {
                epoll.registrations.remove(&guest_fd);
            }
        }
    }

    fn set_block(&mut self, kind: BlockKind, _start_pc: u64, target: Option<u64>) {
        self.block = Some(BlockContext {
            kind,
            target,
            return_target: None,
        });
        self.cond = false;
        self.carg = false;
        self.target = 0;
    }

    fn grow_brk(&mut self, target: u64) {
        let target = align_up(target.max(self.brk_base), PAGE_SIZE);
        let desired_size = target - self.brk_base;
        if let Some(region) = self
            .memory
            .regions
            .iter_mut()
            .find(|region| region.base == self.brk_base)
        {
            if desired_size == 0 {
                region.size = 0;
                region.data.clear();
            } else {
                region.size = desired_size;
                region.data.resize(desired_size as usize, 0);
            }
        } else if desired_size != 0 {
            self.memory.regions.push(runtime::MemoryRegion {
                base: self.brk_base,
                size: desired_size,
                flags: MEM_READ | MEM_WRITE,
                data: vec![0; desired_size as usize],
            });
        }
        self.memory.regions.retain(|region| region.size != 0);
        self.brk_current = target;
    }

    fn alloc_mmap(&mut self, requested: u64, size: u64, prot: u32) -> u64 {
        let base = if requested != 0 {
            align_down(requested, PAGE_SIZE)
        } else {
            let next = align_up(self.mmap_cursor, PAGE_SIZE);
            self.mmap_cursor = next + align_up(size, PAGE_SIZE);
            next
        };
        let size = align_up(size.max(PAGE_SIZE), PAGE_SIZE);
        self.memory.regions.push(runtime::MemoryRegion {
            base,
            size,
            flags: guest_prot_to_region_flags(prot),
            data: vec![0; size as usize],
        });
        base
    }

    fn rlimit_for(&self, resource: u32) -> Option<GuestRlimit> {
        self.rlimits.get(&resource).copied()
    }

    fn set_rlimit(&mut self, resource: u32, limit: GuestRlimit) -> std::result::Result<(), i32> {
        let Some(current) = self.rlimits.get_mut(&resource) else {
            return Err(GUEST_EINVAL);
        };
        if limit.cur > limit.max || limit.max > current.max {
            return Err(GUEST_EINVAL);
        }
        *current = limit;
        Ok(())
    }
}

fn execute_step(
    state: &mut ExecState,
    runtime: &GuestRuntime,
    decoded: &DecodedInstruction,
    commit: &mut CommitRecord,
) -> Result<StepOutcome> {
    let pc = state.pc;
    let fallthrough = pc + decoded.length_bytes() as u64;

    match decoded.mnemonic.as_str() {
        "LUI" => {
            let rd = reg_field(decoded, &["RegDst"])?;
            let imm = sign_extend(need_u(decoded, &["imm20"])? as u64, 20) << 12;
            writeback(state, commit, rd, imm as u64);
        }
        "HL.LUI" | "HL.LIU" => {
            let rd = reg_field(decoded, &["RegDst"])?;
            let imm = need_u(decoded, &["imm", "uimm"])?;
            writeback(state, commit, rd, imm);
        }
        "HL.LIS" => {
            let rd = reg_field(decoded, &["RegDst"])?;
            let imm = field_i(decoded, &["imm", "simm"])? as u64;
            writeback(state, commit, rd, imm);
        }
        "ADDTPC" => {
            let rd = reg_field(decoded, &["RegDst"])?;
            let imm = sign_extend(need_u(decoded, &["imm20"])? as u64, 20) << 12;
            let value = (pc & !0xfff).wrapping_add(imm as u64);
            writeback(state, commit, rd, value);
        }
        "ADD" | "SUB" | "AND" | "OR" | "XOR" => {
            let rd = reg_field(decoded, &["RegDst"])?;
            let lhs_reg = reg_field(decoded, &["SrcL"])?;
            let rhs_reg = reg_field(decoded, &["SrcR"])?;
            let shamt = field_u(decoded, &["shamt"]).unwrap_or(0) as u32;
            let lhs = state.read_reg(lhs_reg);
            let rhs = if decoded.mnemonic == "AND"
                || decoded.mnemonic == "OR"
                || decoded.mnemonic == "XOR"
            {
                apply_src_r_logic(
                    state.read_reg(rhs_reg),
                    field_u(decoded, &["SrcRType"]).unwrap_or(3) as u8,
                    shamt,
                )
            } else {
                apply_src_r_addsub(
                    state.read_reg(rhs_reg),
                    field_u(decoded, &["SrcRType"]).unwrap_or(3) as u8,
                    shamt,
                )
            };
            record_src0(commit, lhs_reg, lhs);
            record_src1(commit, rhs_reg, state.read_reg(rhs_reg));
            let value = match decoded.mnemonic.as_str() {
                "ADD" => lhs.wrapping_add(rhs),
                "SUB" => lhs.wrapping_sub(rhs),
                "AND" => lhs & rhs,
                "OR" => lhs | rhs,
                _ => lhs ^ rhs,
            };
            writeback(state, commit, rd, value);
        }
        "ADDW" | "SUBW" | "ANDW" | "ORW" | "XORW" => {
            let rd = reg_field(decoded, &["RegDst"])?;
            let lhs_reg = reg_field(decoded, &["SrcL"])?;
            let rhs_reg = reg_field(decoded, &["SrcR"])?;
            let shamt = field_u(decoded, &["shamt"]).unwrap_or(0) as u32;
            let lhs = state.read_reg(lhs_reg);
            let rhs = if decoded.mnemonic == "ANDW"
                || decoded.mnemonic == "ORW"
                || decoded.mnemonic == "XORW"
            {
                apply_src_r_logic(
                    state.read_reg(rhs_reg),
                    field_u(decoded, &["SrcRType"]).unwrap_or(3) as u8,
                    shamt,
                )
            } else {
                apply_src_r_addsub(
                    state.read_reg(rhs_reg),
                    field_u(decoded, &["SrcRType"]).unwrap_or(3) as u8,
                    shamt,
                )
            };
            record_src0(commit, lhs_reg, lhs);
            record_src1(commit, rhs_reg, state.read_reg(rhs_reg));
            let value = match decoded.mnemonic.as_str() {
                "ADDW" => lhs.wrapping_add(rhs),
                "SUBW" => lhs.wrapping_sub(rhs),
                "ANDW" => lhs & rhs,
                "ORW" => lhs | rhs,
                _ => lhs ^ rhs,
            };
            writeback(state, commit, rd, sign_extend32(value));
        }
        "ADDI" | "SUBI" | "ADDIW" | "SUBIW" | "HL.ADDI" | "HL.SUBI" | "HL.ADDIW" | "HL.SUBIW" => {
            let rd = reg_field(decoded, &["RegDst"])?;
            let lhs_reg = reg_field(decoded, &["SrcL"])?;
            let lhs = state.read_reg(lhs_reg);
            let imm = need_u(decoded, &["uimm12", "uimm24", "uimm"])?;
            record_src0(commit, lhs_reg, lhs);
            let value = match decoded.mnemonic.as_str() {
                "ADDI" | "ADDIW" | "HL.ADDI" | "HL.ADDIW" => lhs.wrapping_add(imm),
                _ => lhs.wrapping_sub(imm),
            };
            if decoded.mnemonic.ends_with('W') {
                writeback(state, commit, rd, sign_extend32(value));
            } else {
                writeback(state, commit, rd, value);
            }
        }
        "ANDI" | "ORI" | "XORI" | "ANDIW" | "ORIW" | "XORIW" | "HL.ANDI" | "HL.ORI" | "HL.XORI"
        | "HL.ANDIW" | "HL.ORIW" | "HL.XORIW" => {
            let rd = reg_field(decoded, &["RegDst"])?;
            let lhs_reg = reg_field(decoded, &["SrcL"])?;
            let lhs = state.read_reg(lhs_reg);
            let imm = field_i(decoded, &["simm12", "simm24", "simm"])? as u64;
            record_src0(commit, lhs_reg, lhs);
            let value = match decoded.mnemonic.as_str() {
                "ANDI" | "ANDIW" | "HL.ANDI" | "HL.ANDIW" => lhs & imm,
                "ORI" | "ORIW" | "HL.ORI" | "HL.ORIW" => lhs | imm,
                _ => lhs ^ imm,
            };
            if decoded.mnemonic.ends_with('W') {
                writeback(state, commit, rd, sign_extend32(value));
            } else {
                writeback(state, commit, rd, value);
            }
        }
        "SLL" | "SRL" | "SRA" => {
            let rd = reg_field(decoded, &["RegDst"])?;
            let lhs_reg = reg_field(decoded, &["SrcL"])?;
            let rhs_reg = reg_field(decoded, &["SrcR"])?;
            let lhs = state.read_reg(lhs_reg);
            let shamt = (state.read_reg(rhs_reg) & 0x3f) as u32;
            record_src0(commit, lhs_reg, lhs);
            record_src1(commit, rhs_reg, state.read_reg(rhs_reg));
            let value = match decoded.mnemonic.as_str() {
                "SLL" => lhs << shamt,
                "SRL" => lhs >> shamt,
                _ => ((lhs as i64) >> shamt) as u64,
            };
            writeback(state, commit, rd, value);
        }
        "SLLW" | "SRLW" | "SRAW" => {
            let rd = reg_field(decoded, &["RegDst"])?;
            let lhs_reg = reg_field(decoded, &["SrcL"])?;
            let rhs_reg = reg_field(decoded, &["SrcR"])?;
            let lhs = state.read_reg(lhs_reg);
            let shamt = (state.read_reg(rhs_reg) & 0x1f) as u32;
            record_src0(commit, lhs_reg, lhs);
            record_src1(commit, rhs_reg, state.read_reg(rhs_reg));
            let value = match decoded.mnemonic.as_str() {
                "SLLW" => (lhs as u32).wrapping_shl(shamt) as u64,
                "SRLW" => ((lhs as u32) >> shamt) as u64,
                _ => (((lhs as u32) as i32) >> shamt) as u64,
            };
            writeback(state, commit, rd, sign_extend32(value));
        }
        "SLLI" | "SRLI" | "SRAI" => {
            let rd = reg_field(decoded, &["RegDst"])?;
            let lhs_reg = reg_field(decoded, &["SrcL"])?;
            let lhs = state.read_reg(lhs_reg);
            let shamt = (need_u(decoded, &["shamt"])? & 0x3f) as u32;
            record_src0(commit, lhs_reg, lhs);
            let value = match decoded.mnemonic.as_str() {
                "SLLI" => lhs << shamt,
                "SRLI" => lhs >> shamt,
                _ => ((lhs as i64) >> shamt) as u64,
            };
            writeback(state, commit, rd, value);
        }
        "SLLIW" | "SRLIW" | "SRAIW" => {
            let rd = reg_field(decoded, &["RegDst"])?;
            let lhs_reg = reg_field(decoded, &["SrcL"])?;
            let lhs = state.read_reg(lhs_reg);
            let shamt = (need_u(decoded, &["shamt"])? & 0x1f) as u32;
            record_src0(commit, lhs_reg, lhs);
            let value = match decoded.mnemonic.as_str() {
                "SLLIW" => (lhs as u32).wrapping_shl(shamt) as u64,
                "SRLIW" => ((lhs as u32) >> shamt) as u64,
                _ => (((lhs as u32) as i32) >> shamt) as u64,
            };
            writeback(state, commit, rd, sign_extend32(value));
        }
        "MUL" | "DIV" | "DIVU" | "REM" | "REMU" => {
            let rd = reg_field(decoded, &["RegDst"])?;
            let lhs_reg = reg_field(decoded, &["SrcL"])?;
            let rhs_reg = reg_field(decoded, &["SrcR"])?;
            let lhs = state.read_reg(lhs_reg);
            let rhs = state.read_reg(rhs_reg);
            record_src0(commit, lhs_reg, lhs);
            record_src1(commit, rhs_reg, rhs);
            let value = match decoded.mnemonic.as_str() {
                "MUL" => lhs.wrapping_mul(rhs),
                "DIV" => signed_div(lhs, rhs),
                "DIVU" => unsigned_div(lhs, rhs),
                "REM" => signed_rem(lhs, rhs),
                _ => unsigned_rem(lhs, rhs),
            };
            writeback(state, commit, rd, value);
        }
        "MULW" | "DIVW" | "DIVUW" | "REMW" | "REMUW" => {
            let rd = reg_field(decoded, &["RegDst"])?;
            let lhs_reg = reg_field(decoded, &["SrcL"])?;
            let rhs_reg = reg_field(decoded, &["SrcR"])?;
            let lhs = state.read_reg(lhs_reg);
            let rhs = state.read_reg(rhs_reg);
            record_src0(commit, lhs_reg, lhs);
            record_src1(commit, rhs_reg, rhs);
            let lhs32 = lhs as u32;
            let rhs32 = rhs as u32;
            let value = match decoded.mnemonic.as_str() {
                "MULW" => lhs32.wrapping_mul(rhs32) as u64,
                "DIVW" => signed_div(lhs32 as i32 as u64, rhs32 as i32 as u64),
                "DIVUW" => unsigned_div(lhs32 as u64, rhs32 as u64),
                "REMW" => signed_rem(lhs32 as i32 as u64, rhs32 as i32 as u64),
                _ => unsigned_rem(lhs32 as u64, rhs32 as u64),
            };
            writeback(state, commit, rd, sign_extend32(value));
        }
        "CMP.EQ" | "CMP.NE" | "CMP.AND" | "CMP.OR" | "CMP.LT" | "CMP.LTU" | "CMP.GE"
        | "CMP.GEU" => {
            let rd = reg_field(decoded, &["RegDst"])?;
            let lhs_reg = reg_field(decoded, &["SrcL"])?;
            let rhs_reg = reg_field(decoded, &["SrcR"])?;
            let lhs = state.read_reg(lhs_reg);
            let rhs_raw = state.read_reg(rhs_reg);
            record_src0(commit, lhs_reg, lhs);
            record_src1(commit, rhs_reg, rhs_raw);
            let rhs = if decoded.mnemonic == "CMP.AND" || decoded.mnemonic == "CMP.OR" {
                apply_src_r_logic(
                    rhs_raw,
                    field_u(decoded, &["SrcRType"]).unwrap_or(3) as u8,
                    0,
                )
            } else {
                apply_src_r_addsub(
                    rhs_raw,
                    field_u(decoded, &["SrcRType"]).unwrap_or(3) as u8,
                    0,
                )
            };
            let value = match decoded.mnemonic.as_str() {
                "CMP.EQ" => (lhs == rhs) as u64,
                "CMP.NE" => (lhs != rhs) as u64,
                "CMP.AND" => ((lhs & rhs) != 0) as u64,
                "CMP.OR" => ((lhs | rhs) != 0) as u64,
                "CMP.LT" => ((lhs as i64) < (rhs as i64)) as u64,
                "CMP.LTU" => (lhs < rhs) as u64,
                "CMP.GE" => ((lhs as i64) >= (rhs as i64)) as u64,
                _ => (lhs >= rhs) as u64,
            };
            writeback(state, commit, rd, value);
        }
        "CMP.EQI" | "CMP.NEI" | "CMP.ANDI" | "CMP.ORI" | "CMP.LTI" | "CMP.GEI" | "CMP.LTUI"
        | "CMP.GEUI" => {
            let rd = reg_field(decoded, &["RegDst"])?;
            let lhs_reg = reg_field(decoded, &["SrcL"])?;
            let lhs = state.read_reg(lhs_reg);
            record_src0(commit, lhs_reg, lhs);
            let value = match decoded.mnemonic.as_str() {
                "CMP.EQI" => (lhs == field_i(decoded, &["simm12"])? as u64) as u64,
                "CMP.NEI" => (lhs != field_i(decoded, &["simm12"])? as u64) as u64,
                "CMP.ANDI" => ((lhs & field_i(decoded, &["simm12"])? as u64) != 0) as u64,
                "CMP.ORI" => ((lhs | field_i(decoded, &["simm12"])? as u64) != 0) as u64,
                "CMP.LTI" => ((lhs as i64) < field_i(decoded, &["simm12"])? as i64) as u64,
                "CMP.GEI" => ((lhs as i64) >= field_i(decoded, &["simm12"])? as i64) as u64,
                "CMP.LTUI" => (lhs < need_u(decoded, &["uimm12"])? as u64) as u64,
                _ => (lhs >= need_u(decoded, &["uimm12"])? as u64) as u64,
            };
            writeback(state, commit, rd, value);
        }
        "SETC.EQ" | "SETC.NE" | "SETC.AND" | "SETC.OR" | "SETC.LT" | "SETC.LTU" | "SETC.GE"
        | "SETC.GEU" => {
            let lhs_reg = reg_field(decoded, &["SrcL"])?;
            let rhs_reg = reg_field(decoded, &["SrcR"])?;
            let lhs = state.read_reg(lhs_reg);
            let rhs_raw = state.read_reg(rhs_reg);
            record_src0(commit, lhs_reg, lhs);
            record_src1(commit, rhs_reg, rhs_raw);
            let rhs = if decoded.mnemonic == "SETC.AND" || decoded.mnemonic == "SETC.OR" {
                apply_src_r_logic(
                    rhs_raw,
                    field_u(decoded, &["SrcRType"]).unwrap_or(3) as u8,
                    0,
                )
            } else {
                apply_src_r_addsub(
                    rhs_raw,
                    field_u(decoded, &["SrcRType"]).unwrap_or(3) as u8,
                    0,
                )
            };
            state.cond = match decoded.mnemonic.as_str() {
                "SETC.EQ" => lhs == rhs,
                "SETC.NE" => lhs != rhs,
                "SETC.AND" => (lhs & rhs) != 0,
                "SETC.OR" => (lhs | rhs) != 0,
                "SETC.LT" => (lhs as i64) < (rhs as i64),
                "SETC.LTU" => lhs < rhs,
                "SETC.GE" => (lhs as i64) >= (rhs as i64),
                _ => lhs >= rhs,
            };
            state.carg = state.cond;
        }
        "SETC.EQI" | "SETC.NEI" | "SETC.ANDI" | "SETC.ORI" | "SETC.LTI" | "SETC.GEI"
        | "SETC.LTUI" | "SETC.GEUI" => {
            let lhs_reg = reg_field(decoded, &["SrcL"])?;
            let lhs = state.read_reg(lhs_reg);
            record_src0(commit, lhs_reg, lhs);
            state.cond = match decoded.mnemonic.as_str() {
                "SETC.EQI" => lhs == field_i(decoded, &["simm12"])? as u64,
                "SETC.NEI" => lhs != field_i(decoded, &["simm12"])? as u64,
                "SETC.ANDI" => (lhs & field_i(decoded, &["simm12"])? as u64) != 0,
                "SETC.ORI" => (lhs | field_i(decoded, &["simm12"])? as u64) != 0,
                "SETC.LTI" => (lhs as i64) < field_i(decoded, &["simm12"])? as i64,
                "SETC.GEI" => (lhs as i64) >= field_i(decoded, &["simm12"])? as i64,
                "SETC.LTUI" => lhs < need_u(decoded, &["uimm12"])? as u64,
                _ => lhs >= need_u(decoded, &["uimm12"])? as u64,
            };
            state.carg = state.cond;
        }
        "C.SETC.EQ" | "C.SETC.NE" | "C.SETC.LT" | "C.SETC.LTU" | "C.SETC.GE" | "C.SETC.GEU" => {
            let lhs_reg = reg_field(decoded, &["SrcL"])?;
            let rhs_reg = reg_field(decoded, &["SrcR"])?;
            let lhs = state.read_reg(lhs_reg);
            let rhs = state.read_reg(rhs_reg);
            record_src0(commit, lhs_reg, lhs);
            record_src1(commit, rhs_reg, rhs);
            state.cond = match decoded.mnemonic.as_str() {
                "C.SETC.EQ" => lhs == rhs,
                "C.SETC.NE" => lhs != rhs,
                "C.SETC.LT" => (lhs as i64) < (rhs as i64),
                "C.SETC.LTU" => lhs < rhs,
                "C.SETC.GE" => (lhs as i64) >= (rhs as i64),
                _ => lhs >= rhs,
            };
            state.carg = state.cond;
        }
        "SETC.TGT" | "C.SETC.TGT" => {
            let src_reg = reg_field(decoded, &["SrcL"])?;
            let value = state.read_reg(src_reg);
            record_src0(commit, src_reg, value);
            state.target = value;
            state.cond = true;
            state.carg = true;
        }
        "CSEL" => {
            let rd = reg_field(decoded, &["RegDst"])?;
            let pred_reg = reg_field(decoded, &["SrcP"])?;
            let lhs_reg = reg_field(decoded, &["SrcL"])?;
            let rhs_reg = reg_field(decoded, &["SrcR"])?;
            let pred = state.read_reg(pred_reg);
            let lhs = state.read_reg(lhs_reg);
            let rhs = apply_src_r_addsub(
                state.read_reg(rhs_reg),
                field_u(decoded, &["SrcRType"]).unwrap_or(3) as u8,
                0,
            );
            record_src0(commit, lhs_reg, lhs);
            record_src1(commit, rhs_reg, state.read_reg(rhs_reg));
            let value = if pred != 0 { rhs } else { lhs };
            writeback(state, commit, rd, value);
        }
        "LBI" | "LBUI" | "LHI" | "LHUI" | "LWI" | "LWUI" | "LDI" => {
            let rd = reg_field(decoded, &["RegDst"])?;
            let base_reg = reg_field(decoded, &["SrcL"])?;
            let base = state.read_reg(base_reg);
            let scale = match decoded.mnemonic.as_str() {
                "LHI" | "LHUI" => 2,
                "LWI" | "LWUI" => 4,
                "LDI" => 8,
                _ => 1,
            };
            let offset = field_i(decoded, &["simm12"])? * scale;
            let addr = base.wrapping_add(offset as u64);
            record_src0(commit, base_reg, base);
            let value = load_value(&state.memory, decoded.mnemonic.as_str(), addr)?;
            record_load(commit, addr, value.raw, value.size);
            writeback(state, commit, rd, value.value);
        }
        "LB" | "LBU" | "LH" | "LHU" | "LW" | "LWU" | "LD" => {
            let rd = reg_field(decoded, &["RegDst"])?;
            let base_reg = reg_field(decoded, &["SrcL"])?;
            let idx_reg = reg_field(decoded, &["SrcR"])?;
            let base = state.read_reg(base_reg);
            let idx_raw = state.read_reg(idx_reg);
            let shamt = field_u(decoded, &["shamt"]).unwrap_or(0) as u32;
            let idx = apply_src_r_addsub(
                idx_raw,
                field_u(decoded, &["SrcRType"]).unwrap_or(3) as u8,
                shamt,
            );
            let addr = base.wrapping_add(idx);
            record_src0(commit, base_reg, base);
            record_src1(commit, idx_reg, idx_raw);
            let value = load_value(&state.memory, decoded.mnemonic.as_str(), addr)?;
            record_load(commit, addr, value.raw, value.size);
            writeback(state, commit, rd, value.value);
        }
        "HL.LBIP" | "HL.LBUIP" | "HL.LHIP" | "HL.LHIP.U" | "HL.LHUIP" | "HL.LHUIP.U"
        | "HL.LWIP" | "HL.LWIP.U" | "HL.LWUIP" | "HL.LWUIP.U" | "HL.LDIP" | "HL.LDIP.U" => {
            let rd0 = reg_field(decoded, &["RegDst0"])?;
            let rd1 = reg_field(decoded, &["RegDst1"])?;
            let base_reg = reg_field(decoded, &["SrcL"])?;
            let base = state.read_reg(base_reg);
            let pair = pair_access(decoded.mnemonic.as_str())?;
            let simm = field_i(decoded, &["simm17"])?;
            let offset = if pair.unscaled {
                simm
            } else {
                simm * i64::from(pair.elem_size)
            };
            let addr0 = base.wrapping_add(offset as u64);
            let addr1 = addr0.wrapping_add(u64::from(pair.elem_size));
            let value0 = load_value(&state.memory, pair.load_mnemonic, addr0)?;
            let value1 = load_value(&state.memory, pair.load_mnemonic, addr1)?;
            record_src0(commit, base_reg, base);
            record_load(commit, addr0, value0.raw, pair.elem_size.saturating_mul(2));
            state.write_reg(rd1, value1.value);
            writeback(state, commit, rd0, value0.value);
        }
        "SBI" | "SHI" | "SWI" | "SDI" => {
            let src_reg = reg_field(decoded, &["SrcL"])?;
            let base_reg = reg_field(decoded, &["SrcR"])?;
            let src_value = state.read_reg(src_reg);
            let base = state.read_reg(base_reg);
            let scale = match decoded.mnemonic.as_str() {
                "SHI" => 2,
                "SWI" => 4,
                "SDI" => 8,
                _ => 1,
            };
            let offset = field_i(decoded, &["simm12"])? * scale;
            let addr = base.wrapping_add(offset as u64);
            record_src0(commit, src_reg, src_value);
            record_src1(commit, base_reg, base);
            store_value(
                &mut state.memory,
                decoded.mnemonic.as_str(),
                addr,
                src_value,
            )?;
            record_store(
                commit,
                addr,
                src_value,
                size_for_store(decoded.mnemonic.as_str()),
            );
        }
        "SB" | "SH" | "SW" | "SD" => {
            let src_reg = reg_field(decoded, &["SrcD", "SrcP"])?;
            let base_reg = reg_field(decoded, &["SrcL"])?;
            let idx_reg = reg_field(decoded, &["SrcR"])?;
            let src_value = state.read_reg(src_reg);
            let base = state.read_reg(base_reg);
            let idx_raw = state.read_reg(idx_reg);
            let shamt = field_u(decoded, &["shamt"])
                .unwrap_or(size_shift_for_store(decoded.mnemonic.as_str()) as u64)
                as u32;
            let idx = apply_src_r_addsub(
                idx_raw,
                field_u(decoded, &["SrcRType"]).unwrap_or(3) as u8,
                shamt,
            );
            let addr = base.wrapping_add(idx);
            record_src0(commit, src_reg, src_value);
            record_src1(commit, base_reg, base);
            store_value(
                &mut state.memory,
                decoded.mnemonic.as_str(),
                addr,
                src_value,
            )?;
            record_store(
                commit,
                addr,
                src_value,
                size_for_store(decoded.mnemonic.as_str()),
            );
        }
        "HL.SBIP" | "HL.SHIP" | "HL.SHIP.U" | "HL.SWIP" | "HL.SWIP.U" | "HL.SDIP" | "HL.SDIP.U" => {
            let src0_reg = reg_field(decoded, &["SrcD"])?;
            let src1_reg = reg_field(decoded, &["SrcD1"])?;
            let base_reg = reg_field(decoded, &["SrcR"])?;
            let src0_value = state.read_reg(src0_reg);
            let src1_value = state.read_reg(src1_reg);
            let base = state.read_reg(base_reg);
            let pair = pair_access(decoded.mnemonic.as_str())?;
            let simm = field_i(decoded, &["simm17"])?;
            let offset = if pair.unscaled {
                simm
            } else {
                simm * i64::from(pair.elem_size)
            };
            let addr0 = base.wrapping_add(offset as u64);
            let addr1 = addr0.wrapping_add(u64::from(pair.elem_size));
            record_src0(commit, src0_reg, src0_value);
            record_src1(commit, src1_reg, src1_value);
            store_value(&mut state.memory, pair.store_mnemonic, addr0, src0_value)?;
            store_value(&mut state.memory, pair.store_mnemonic, addr1, src1_value)?;
            record_store(commit, addr0, src0_value, pair.elem_size.saturating_mul(2));
        }
        "SSRGET" => {
            let rd = reg_field(decoded, &["RegDst"])?;
            let ssr = need_u(decoded, &["SSR_ID"])? as u16;
            let value = *state.ssr.get(&ssr).unwrap_or(&0);
            writeback(state, commit, rd, value);
        }
        "SSRSET" => {
            let src_reg = reg_field(decoded, &["SrcL"])?;
            let ssr = need_u(decoded, &["SSR_ID"])? as u16;
            let value = state.read_reg(src_reg);
            record_src0(commit, src_reg, value);
            state.ssr.insert(ssr, value);
        }
        "C.SSRGET" => {
            let ssr = need_u(decoded, &["SrcL"])? as u16;
            let value = *state.ssr.get(&ssr).unwrap_or(&0);
            writeback(state, commit, REG_IMPLICIT_T_DST, value);
        }
        "C.MOVR" => {
            let rd = reg_field(decoded, &["RegDst"])?;
            let src_reg = reg_field(decoded, &["SrcL"])?;
            let value = state.read_reg(src_reg);
            record_src0(commit, src_reg, value);
            writeback(state, commit, rd, value);
        }
        "C.MOVI" => {
            let rd = reg_field(decoded, &["RegDst"])?;
            let value = field_i(decoded, &["simm5"])? as u64;
            writeback(state, commit, rd, value);
        }
        "C.ADDI" => {
            let lhs_reg = reg_field(decoded, &["SrcL"])?;
            let lhs = state.read_reg(lhs_reg);
            let value = lhs.wrapping_add(field_i(decoded, &["simm5"])? as u64);
            record_src0(commit, lhs_reg, lhs);
            writeback(state, commit, REG_IMPLICIT_T_DST, value);
        }
        "C.ADD" | "C.SUB" | "C.AND" | "C.OR" => {
            let lhs_reg = reg_field(decoded, &["SrcL"])?;
            let rhs_reg = reg_field(decoded, &["SrcR"])?;
            let lhs = state.read_reg(lhs_reg);
            let rhs = state.read_reg(rhs_reg);
            record_src0(commit, lhs_reg, lhs);
            record_src1(commit, rhs_reg, rhs);
            let value = match decoded.mnemonic.as_str() {
                "C.ADD" => lhs.wrapping_add(rhs),
                "C.SUB" => lhs.wrapping_sub(rhs),
                "C.AND" => lhs & rhs,
                _ => lhs | rhs,
            };
            writeback(state, commit, REG_IMPLICIT_T_DST, value);
        }
        "C.SLLI" | "C.SRLI" => {
            let value = state.read_reg(REG_T1);
            let shamt = (need_u(decoded, &["uimm5"])? & 0x1f) as u32;
            record_src0(commit, REG_T1, value);
            let out = if decoded.mnemonic == "C.SLLI" {
                value << shamt
            } else {
                value >> shamt
            };
            writeback(state, commit, REG_IMPLICIT_T_DST, out);
        }
        "C.ZEXT.B" | "C.ZEXT.H" | "C.ZEXT.W" | "C.SEXT.B" | "C.SEXT.H" | "C.SEXT.W" => {
            let src_reg = reg_field(decoded, &["SrcL"])?;
            let value = state.read_reg(src_reg);
            record_src0(commit, src_reg, value);
            let out = match decoded.mnemonic.as_str() {
                "C.ZEXT.B" => value as u8 as u64,
                "C.ZEXT.H" => value as u16 as u64,
                "C.ZEXT.W" => value as u32 as u64,
                "C.SEXT.B" => (value as u8 as i8 as i64) as u64,
                "C.SEXT.H" => (value as u16 as i16 as i64) as u64,
                _ => sign_extend32(value),
            };
            writeback(state, commit, REG_IMPLICIT_T_DST, out);
        }
        "C.LWI" | "C.LDI" => {
            let base_reg = reg_field(decoded, &["SrcL"])?;
            let base = state.read_reg(base_reg);
            let scale = if decoded.mnemonic == "C.LDI" { 8 } else { 4 };
            let addr = base.wrapping_add((field_i(decoded, &["simm5"])? * scale) as u64);
            record_src0(commit, base_reg, base);
            let value = load_value(
                &state.memory,
                if decoded.mnemonic == "C.LDI" {
                    "LD"
                } else {
                    "LW"
                },
                addr,
            )?;
            record_load(commit, addr, value.raw, value.size);
            writeback(state, commit, REG_IMPLICIT_T_DST, value.value);
        }
        "C.SWI" | "C.SDI" => {
            let base_reg = reg_field(decoded, &["SrcL"])?;
            let base = state.read_reg(base_reg);
            let src_value = state.read_reg(REG_T1);
            let scale = if decoded.mnemonic == "C.SDI" { 8 } else { 4 };
            let addr = base.wrapping_add((field_i(decoded, &["simm5"])? * scale) as u64);
            record_src0(commit, REG_T1, src_value);
            record_src1(commit, base_reg, base);
            store_value(
                &mut state.memory,
                if decoded.mnemonic == "C.SDI" {
                    "SD"
                } else {
                    "SW"
                },
                addr,
                src_value,
            )?;
            record_store(
                commit,
                addr,
                src_value,
                if decoded.mnemonic == "C.SDI" { 8 } else { 4 },
            );
        }
        "SETRET" => {
            let target = pc.wrapping_add(need_u(decoded, &["imm20"])? << 1);
            writeback(state, commit, REG_RA, target);
            if let Some(block) = &mut state.block {
                block.return_target = Some(target);
            }
        }
        "C.SETRET" => {
            let target = pc.wrapping_add(need_u(decoded, &["uimm5"])? << 1);
            writeback(state, commit, REG_RA, target);
            if let Some(block) = &mut state.block {
                block.return_target = Some(target);
            }
        }
        "HL.SETRET" => {
            let offset = sign_extend(need_u(decoded, &["imm32"])? as u64, 32) as i128;
            let target = pc.wrapping_add((offset << 1) as u64);
            writeback(state, commit, REG_RA, target);
            if let Some(block) = &mut state.block {
                block.return_target = Some(target);
            }
        }
        "J" => {
            let target = pc.wrapping_add(((field_i(decoded, &["simm22"])? as i128) << 1) as u64);
            commit.next_pc = target;
            return Ok(StepOutcome {
                next_pc: target,
                exit: None,
                retire_cause: "jump".to_string(),
            });
        }
        "JR" => {
            let base_reg = reg_field(decoded, &["SrcL"])?;
            let base = state.read_reg(base_reg);
            record_src0(commit, base_reg, base);
            let target = base.wrapping_add(((field_i(decoded, &["simm12"])? as i128) << 1) as u64);
            commit.next_pc = target;
            return Ok(StepOutcome {
                next_pc: target,
                exit: None,
                retire_cause: "jump_reg".to_string(),
            });
        }
        "BSTART.STD" | "HL.BSTART.STD" | "HL.BSTART.FP" | "HL.BSTART.SYS" => {
            let kind = match decoded.asm.as_str() {
                asm if asm.contains("CALL") => BlockKind::Call,
                asm if asm.contains("DIRECT") => BlockKind::Direct,
                asm if asm.contains("COND") => BlockKind::Cond,
                asm if asm.contains("RET") => BlockKind::Ret,
                asm if asm.contains("ICALL") => BlockKind::ICall,
                asm if asm.contains("IND") => BlockKind::Ind,
                _ => BlockKind::Fall,
            };
            let target = if matches!(kind, BlockKind::Call | BlockKind::Cond | BlockKind::Direct) {
                Some(match decoded.mnemonic.as_str() {
                    "BSTART.STD" => {
                        pc.wrapping_add(((field_i(decoded, &["simm17"])? as i128) << 1) as u64)
                    }
                    _ => pc.wrapping_add(field_i(decoded, &["simm"])? as u64),
                })
            } else {
                None
            };
            state.set_block(kind, pc, target);
        }
        "BSTART CALL" | "HL.BSTART.CALL" => {
            let (target, return_target) = match decoded.mnemonic.as_str() {
                "BSTART CALL" => (
                    pc.wrapping_add(((field_i(decoded, &["simm12"])? as i128) << 1) as u64),
                    pc.wrapping_add(need_u(decoded, &["uimm5"])? << 1),
                ),
                _ => (
                    pc.wrapping_add(field_i(decoded, &["simm25"])? as u64),
                    fallthrough,
                ),
            };
            state.set_block(BlockKind::Call, pc, Some(target));
            state.write_reg(REG_RA, return_target);
            if let Some(block) = &mut state.block {
                block.return_target = Some(return_target);
            }
            commit.wb_valid = 1;
            commit.wb_rd = REG_RA as u8;
            commit.wb_data = return_target;
            commit.dst_valid = 1;
            commit.dst_reg = REG_RA as u8;
            commit.dst_data = return_target;
        }
        "C.BSTART" => {
            let kind = if decoded.asm.contains("COND") {
                BlockKind::Cond
            } else {
                BlockKind::Direct
            };
            let target = pc.wrapping_add(((field_i(decoded, &["simm12"])? as i128) << 1) as u64);
            state.set_block(kind, pc, Some(target));
        }
        "C.BSTART.STD" => {
            let br_type = need_u(decoded, &["BrType"])? as u8;
            let kind = match br_type {
                5 => BlockKind::Ind,
                6 => BlockKind::ICall,
                7 => BlockKind::Ret,
                _ => BlockKind::Fall,
            };
            state.set_block(kind, pc, None);
        }
        "C.BSTOP" => {
            let next_pc = resolve_block_end(state, fallthrough);
            commit.next_pc = next_pc;
            return Ok(StepOutcome {
                next_pc,
                exit: None,
                retire_cause: "block_end".to_string(),
            });
        }
        "FENTRY" => {
            apply_fentry(
                state,
                commit,
                need_u(decoded, &["SrcBegin"])? as usize,
                need_u(decoded, &["SrcEnd"])? as usize,
                need_u(decoded, &["uimm"])?,
            )?;
        }
        "FEXIT" => {
            apply_fexit(
                state,
                commit,
                need_u(decoded, &["DstBegin"])? as usize,
                need_u(decoded, &["DstEnd"])? as usize,
                need_u(decoded, &["uimm"])?,
            )?;
        }
        "FRET.STK" => {
            let target = apply_fret_stk(
                state,
                commit,
                need_u(decoded, &["DstBegin"])? as usize,
                need_u(decoded, &["DstEnd"])? as usize,
                need_u(decoded, &["uimm"])?,
            )?;
            commit.next_pc = target;
            return Ok(StepOutcome {
                next_pc: target,
                exit: None,
                retire_cause: "fret_stk".to_string(),
            });
        }
        "FRET.RA" => {
            let target = apply_fret_ra(
                state,
                commit,
                need_u(decoded, &["DstBegin"])? as usize,
                need_u(decoded, &["DstEnd"])? as usize,
                need_u(decoded, &["uimm"])?,
            )?;
            commit.next_pc = target;
            return Ok(StepOutcome {
                next_pc: target,
                exit: None,
                retire_cause: "fret_ra".to_string(),
            });
        }
        "ACRC" => {
            let rst = need_u(decoded, &["RST_Type"])?;
            if rst != 1 {
                bail!("unsupported ACRC rst_type {rst}");
            }
            let outcome = dispatch_syscall(state, runtime, commit)?;
            commit.next_pc = fallthrough;
            return Ok(StepOutcome {
                next_pc: fallthrough,
                exit: outcome,
                retire_cause: "syscall".to_string(),
            });
        }
        "EBREAK" | "C.EBREAK" => {
            commit.trap_valid = 1;
            commit.trap_cause = TRAP_SW_BREAKPOINT;
            commit.traparg0 = decoded.instruction_bits;
            commit.next_pc = fallthrough;
            return Ok(StepOutcome {
                next_pc: fallthrough,
                exit: Some(ExitSignal::Breakpoint),
                retire_cause: "breakpoint".to_string(),
            });
        }
        "C.BSTART.STD RET" => unreachable!(),
        other => bail!("unsupported mnemonic {other}"),
    }

    commit.next_pc = fallthrough;
    Ok(StepOutcome {
        next_pc: fallthrough,
        exit: None,
        retire_cause: "execute".to_string(),
    })
}

fn dispatch_syscall(
    state: &mut ExecState,
    runtime: &GuestRuntime,
    commit: &mut CommitRecord,
) -> Result<Option<ExitSignal>> {
    let number = state.read_reg(REG_A7);
    let args = [
        state.read_reg(REG_A0),
        state.read_reg(REG_A1),
        state.read_reg(REG_A2),
        state.read_reg(REG_A3),
        state.read_reg(REG_A4),
        state.read_reg(REG_A5),
    ];
    record_src0(commit, REG_A0, args[0]);
    record_src1(commit, REG_A7, number);

    let result = match number {
        SYS_EVENTFD2 => dispatch_eventfd2(state, args),
        SYS_EPOLL_CREATE1 => dispatch_epoll_create1(state, args),
        SYS_EPOLL_CTL => dispatch_epoll_ctl(state, args),
        SYS_EPOLL_PWAIT => dispatch_epoll_pwait(state, commit, args),
        SYS_GETPID => Ok(state.current_pid),
        SYS_GETPPID => Ok(state.current_ppid),
        SYS_WAIT4 => dispatch_wait4(state, args),
        SYS_GETCWD => dispatch_getcwd(state, runtime, args),
        SYS_PSELECT6 => dispatch_pselect6(state, commit, args),
        SYS_PPOLL => dispatch_ppoll(state, commit, args),
        SYS_PRCTL => dispatch_prctl(state, commit, args),
        SYS_GETUID => Ok(state.uid as u64),
        SYS_GETEUID => Ok(state.euid as u64),
        SYS_GETGID => Ok(state.gid as u64),
        SYS_GETEGID => Ok(state.egid as u64),
        SYS_GETRESUID => match dispatch_getres_ids(
            &mut state.memory,
            [args[0], args[1], args[2]],
            [state.uid, state.euid, state.suid],
        ) {
            Ok(()) => {
                record_store(commit, args[0], state.uid as u64, 12);
                Ok(0)
            }
            Err(errno) => Err(errno),
        },
        SYS_GETRESGID => match dispatch_getres_ids(
            &mut state.memory,
            [args[0], args[1], args[2]],
            [state.gid, state.egid, state.sgid],
        ) {
            Ok(()) => {
                record_store(commit, args[0], state.gid as u64, 12);
                Ok(0)
            }
            Err(errno) => Err(errno),
        },
        SYS_SETUID => match validate_single_id(args[0], &[state.uid, state.euid, state.suid]) {
            Ok(uid) => {
                state.uid = uid;
                state.euid = uid;
                state.suid = uid;
                Ok(0)
            }
            Err(errno) => Err(errno),
        },
        SYS_SETGID => match validate_single_id(args[0], &[state.gid, state.egid, state.sgid]) {
            Ok(gid) => {
                state.gid = gid;
                state.egid = gid;
                state.sgid = gid;
                Ok(0)
            }
            Err(errno) => Err(errno),
        },
        SYS_SETRESUID => match apply_setres_ids(
            [state.uid, state.euid, state.suid],
            [args[0], args[1], args[2]],
        ) {
            Ok([uid, euid, suid]) => {
                state.uid = uid;
                state.euid = euid;
                state.suid = suid;
                Ok(0)
            }
            Err(errno) => Err(errno),
        },
        SYS_SETRESGID => match apply_setres_ids(
            [state.gid, state.egid, state.sgid],
            [args[0], args[1], args[2]],
        ) {
            Ok([gid, egid, sgid]) => {
                state.gid = gid;
                state.egid = egid;
                state.sgid = sgid;
                Ok(0)
            }
            Err(errno) => Err(errno),
        },
        SYS_GETTID => Ok(state.current_pid),
        SYS_GETRANDOM => dispatch_getrandom(state, commit, args),
        SYS_MEMBARRIER => dispatch_membarrier(state, args),
        SYS_RSEQ => dispatch_rseq(state, commit, args),
        SYS_SIGALTSTACK => dispatch_sigaltstack(state, commit, args),
        SYS_SET_TID_ADDRESS => {
            state.clear_child_tid = args[0];
            Ok(state.current_pid)
        }
        SYS_SET_ROBUST_LIST => {
            state.robust_list_head = args[0];
            state.robust_list_len = args[1];
            Ok(0)
        }
        SYS_FUTEX => dispatch_futex(state, args),
        SYS_UNAME => {
            if write_guest_utsname(&mut state.memory, args[0]).is_err() {
                Err(GUEST_EFAULT)
            } else {
                record_store(commit, args[0], 0, trace_size(GUEST_UTSNAME_SIZE));
                Ok(0)
            }
        }
        SYS_SYSINFO => {
            if write_guest_sysinfo(&mut state.memory, args[0], runtime).is_err() {
                Err(GUEST_EFAULT)
            } else {
                record_store(commit, args[0], 0, trace_size(GUEST_SYSINFO_SIZE));
                Ok(0)
            }
        }
        SYS_PRLIMIT64 => dispatch_prlimit64(state, args),
        SYS_CLOCK_GETTIME => {
            let clk_id: libc::clockid_t = args[0].try_into().unwrap_or(libc::CLOCK_REALTIME);
            let mut ts = timespec {
                tv_sec: 0,
                tv_nsec: 0,
            };
            let rc = unsafe { clock_gettime(clk_id, &mut ts as *mut timespec) };
            if rc != 0 {
                Err(last_errno())
            } else {
                if state
                    .memory
                    .write_u64_checked(args[1], ts.tv_sec as u64)
                    .is_none()
                    || state
                        .memory
                        .write_u64_checked(args[1] + 8, ts.tv_nsec as u64)
                        .is_none()
                {
                    Err(GUEST_EFAULT)
                } else {
                    record_store(commit, args[1], ts.tv_sec as u64, 16);
                    Ok(0)
                }
            }
        }
        SYS_FCNTL => dispatch_fcntl(state, args),
        SYS_IOCTL => dispatch_ioctl(state, commit, args),
        SYS_WRITE => match state.special_fds.get_mut(&args[0]) {
            Some(SpecialFdKind::EventFd(eventfd)) => {
                dispatch_eventfd_write(&mut state.memory, eventfd, args)
            }
            Some(SpecialFdKind::Epoll(_)) => Err(GUEST_EINVAL),
            None => match state.host_fd(args[0]) {
                Ok(host_fd) => {
                    let Some(bytes) = state.memory.read_bytes_checked(args[1], args[2] as usize)
                    else {
                        return finalize_syscall(state, commit, Err(GUEST_EFAULT));
                    };
                    let rc = unsafe { libc::write(host_fd, bytes.as_ptr().cast(), bytes.len()) };
                    if rc < 0 {
                        Err(last_errno())
                    } else {
                        Ok(rc as u64)
                    }
                }
                Err(errno) => Err(errno),
            },
        },
        SYS_READ => match state.special_fds.get(&args[0]) {
            Some(SpecialFdKind::EventFd(_)) => {
                let read_fd = match state.host_fd(args[0]) {
                    Ok(read_fd) => read_fd,
                    Err(errno) => return finalize_syscall(state, commit, Err(errno)),
                };
                let Some(SpecialFdKind::EventFd(eventfd)) = state.special_fds.get_mut(&args[0])
                else {
                    unreachable!();
                };
                dispatch_eventfd_read(&mut state.memory, read_fd, eventfd, commit, args)
            }
            Some(SpecialFdKind::Epoll(_)) => Err(GUEST_EINVAL),
            None => match state.host_fd(args[0]) {
                Ok(host_fd) => {
                    let mut bytes = vec![0u8; args[2] as usize];
                    let rc = unsafe { libc::read(host_fd, bytes.as_mut_ptr().cast(), bytes.len()) };
                    if rc < 0 {
                        Err(last_errno())
                    } else {
                        let count = rc as usize;
                        if state
                            .memory
                            .write_bytes_checked(args[1], &bytes[..count])
                            .is_none()
                        {
                            return finalize_syscall(state, commit, Err(GUEST_EFAULT));
                        }
                        record_load(commit, args[1], 0, trace_size(count));
                        Ok(count as u64)
                    }
                }
                Err(errno) => Err(errno),
            },
        },
        SYS_DUP3 => dispatch_dup3(state, args),
        SYS_OPENAT => {
            let dirfd = args[0] as i64 as i32;
            let Some(path) = state.memory.read_c_string_checked(args[1], MAX_C_STRING) else {
                return finalize_syscall(state, commit, Err(GUEST_EFAULT));
            };
            let flags = args[2] as i32;
            let mode = args[3] as libc::mode_t;
            let resolved = resolve_open_path(runtime, dirfd, &path);
            let c_path = CString::new(resolved.as_str()).context("guest open path contains NUL")?;
            let host_fd = if dirfd == GUEST_AT_FDCWD || resolved != path {
                unsafe { libc::open(c_path.as_ptr(), flags, mode as libc::c_uint) }
            } else {
                match state.host_fd(args[0]) {
                    Ok(host_dirfd) => unsafe {
                        libc::openat(host_dirfd, c_path.as_ptr(), flags, mode as libc::c_uint)
                    },
                    Err(errno) => return finalize_syscall(state, commit, Err(errno)),
                }
            };
            if host_fd < 0 {
                Err(last_errno())
            } else {
                let guest_fd = state.alloc_guest_fd();
                state.insert_guest_fd(guest_fd, host_fd, flags & !GUEST_O_CLOEXEC, 0);
                Ok(guest_fd)
            }
        }
        SYS_CLOSE => state.close_guest_fd(args[0]),
        SYS_PIPE2 => dispatch_pipe2(state, commit, args),
        SYS_LSEEK => match state.host_fd(args[0]) {
            Ok(host_fd) => {
                let offset = args[1] as i64;
                let whence = args[2] as i32;
                let rc = unsafe { libc::lseek(host_fd, offset, whence) };
                if rc < 0 {
                    Err(last_errno())
                } else {
                    Ok(rc as u64)
                }
            }
            Err(errno) => Err(errno),
        },
        SYS_FSTAT => match state.host_fd(args[0]) {
            Ok(host_fd) => {
                let stat = match host_fstat(host_fd) {
                    Ok(stat) => stat,
                    Err(errno) => return finalize_syscall(state, commit, Err(errno)),
                };
                if write_guest_linux_stat(&mut state.memory, args[1], stat).is_err() {
                    return finalize_syscall(state, commit, Err(GUEST_EFAULT));
                }
                record_store(
                    commit,
                    args[1],
                    stat.size as u64,
                    GUEST_LINUX_STAT_SIZE as u8,
                );
                Ok(0)
            }
            Err(errno) => Err(errno),
        },
        SYS_NEWFSTATAT => dispatch_newfstatat(state, runtime, commit, args),
        SYS_READLINKAT => dispatch_readlinkat(state, runtime, commit, args),
        SYS_BRK => {
            if args[0] != 0 {
                state.grow_brk(args[0]);
            }
            Ok(state.brk_current)
        }
        SYS_MADVISE => dispatch_madvise(state, args),
        SYS_MMAP => {
            if args[1] == 0 {
                Err(GUEST_EINVAL)
            } else {
                let addr = state.alloc_mmap(args[0], args[1], args[2] as u32);
                Ok(addr)
            }
        }
        SYS_MUNMAP => {
            let addr = args[0];
            let size = align_up(args[1], PAGE_SIZE);
            if size == 0 || addr & (PAGE_SIZE - 1) != 0 {
                Err(GUEST_EINVAL)
            } else {
                state.memory.unmap_range(addr, size);
                Ok(0)
            }
        }
        SYS_MPROTECT => {
            let addr = args[0];
            let size = align_up(args[1], PAGE_SIZE);
            if size == 0 || addr & (PAGE_SIZE - 1) != 0 {
                Err(GUEST_EINVAL)
            } else if state.memory.protect_range(
                addr,
                size,
                guest_prot_to_region_flags(args[2] as u32),
            ) {
                Ok(0)
            } else {
                Err(GUEST_ENOMEM)
            }
        }
        SYS_RT_SIGACTION => {
            if args[2] != 0 {
                let bytes = vec![0u8; 32];
                if state.memory.write_bytes_checked(args[2], &bytes).is_none() {
                    Err(GUEST_EFAULT)
                } else {
                    record_store(commit, args[2], 0, trace_size(bytes.len()));
                    Ok(0)
                }
            } else {
                Ok(0)
            }
        }
        SYS_RT_SIGPROCMASK => {
            let size = args[3] as usize;
            if size == 0 || size > GUEST_SIGSET_BYTES {
                Err(GUEST_EINVAL)
            } else if args[2] != 0 {
                let bytes = vec![0u8; size];
                if state.memory.write_bytes_checked(args[2], &bytes).is_none() {
                    Err(GUEST_EFAULT)
                } else {
                    record_store(commit, args[2], 0, trace_size(size));
                    Ok(0)
                }
            } else {
                Ok(0)
            }
        }
        SYS_EXIT | SYS_EXIT_GROUP => {
            return Ok(Some(ExitSignal::GuestExit(args[0] as i32)));
        }
        _ => Err(GUEST_ENOSYS),
    };

    finalize_syscall(state, commit, result)
}

fn resolve_block_end(state: &mut ExecState, fallthrough: u64) -> u64 {
    let block = state.block.take();
    let next_pc = match block {
        None => fallthrough,
        Some(block) => match block.kind {
            BlockKind::Fall => fallthrough,
            BlockKind::Direct => block.target.unwrap_or(fallthrough),
            BlockKind::Cond => {
                if state.cond {
                    block.target.unwrap_or(fallthrough)
                } else {
                    fallthrough
                }
            }
            BlockKind::Call => {
                if block.return_target.is_none() {
                    state.write_reg(REG_RA, fallthrough);
                }
                block.target.unwrap_or(fallthrough)
            }
            BlockKind::Ind => state.target,
            BlockKind::ICall => {
                if block.return_target.is_none() {
                    state.write_reg(REG_RA, fallthrough);
                }
                state.target
            }
            BlockKind::Ret => {
                if state.target != 0 {
                    state.target
                } else {
                    state.read_reg(REG_RA)
                }
            }
        },
    };
    state.cond = false;
    state.carg = false;
    state.target = 0;
    next_pc
}

fn apply_fentry(
    state: &mut ExecState,
    commit: &mut CommitRecord,
    begin: usize,
    end: usize,
    stack_size: u64,
) -> Result<()> {
    let old_sp = state.read_reg(REG_SP);
    let new_sp = old_sp
        .checked_sub(stack_size)
        .context("stack underflow in FENTRY")?;
    let regs = wrapped_reg_sequence(begin, end);
    for (idx, reg) in regs.into_iter().enumerate() {
        if reg == REG_ZERO {
            continue;
        }
        let offset = stack_size
            .checked_sub(((idx + 1) as u64) * 8)
            .context("invalid FENTRY stack frame size")?;
        state
            .memory
            .write_u64(new_sp + offset, state.read_reg(reg))
            .context("failed to save register during FENTRY")?;
    }
    writeback(state, commit, REG_SP, new_sp);
    Ok(())
}

fn apply_fexit(
    state: &mut ExecState,
    commit: &mut CommitRecord,
    begin: usize,
    end: usize,
    stack_size: u64,
) -> Result<()> {
    let new_sp = state.read_reg(REG_SP).wrapping_add(stack_size);
    let regs = wrapped_reg_sequence(begin, end);
    for (idx, reg) in regs.into_iter().enumerate() {
        if reg == REG_ZERO {
            continue;
        }
        let value = state
            .memory
            .read_u64(new_sp - (((idx + 1) as u64) * 8))
            .context("failed to restore register during FEXIT")?;
        state.write_reg(reg, value);
    }
    writeback(state, commit, REG_SP, new_sp);
    Ok(())
}

fn apply_fret_stk(
    state: &mut ExecState,
    commit: &mut CommitRecord,
    begin: usize,
    end: usize,
    stack_size: u64,
) -> Result<u64> {
    apply_fexit(state, commit, begin, end, stack_size)?;
    let target = state.read_reg(REG_RA);
    state.block = None;
    state.cond = false;
    state.carg = false;
    state.target = 0;
    Ok(target)
}

fn apply_fret_ra(
    state: &mut ExecState,
    commit: &mut CommitRecord,
    begin: usize,
    end: usize,
    stack_size: u64,
) -> Result<u64> {
    let target = state.read_reg(REG_RA);
    apply_fexit(state, commit, begin, end, stack_size)?;
    state.block = None;
    state.cond = false;
    state.carg = false;
    state.target = 0;
    Ok(target)
}

fn wrapped_reg_sequence(begin: usize, end: usize) -> Vec<usize> {
    let mut regs = Vec::new();
    let mut current = begin;
    loop {
        regs.push(current);
        if current == end {
            break;
        }
        current += 1;
        if current > 23 {
            current = 2;
        }
    }
    regs
}

struct LoadResult {
    value: u64,
    raw: u64,
    size: u8,
}

struct PairAccess {
    load_mnemonic: &'static str,
    store_mnemonic: &'static str,
    elem_size: u8,
    unscaled: bool,
}

fn load_value(memory: &GuestMemory, mnemonic: &str, addr: u64) -> Result<LoadResult> {
    let result = match mnemonic {
        "LB" | "LBI" => {
            let raw = memory.read_u8_checked(addr).context("faulting LB")?;
            LoadResult {
                value: sign_extend(raw as u64, 8) as u64,
                raw: raw as u64,
                size: 1,
            }
        }
        "LBU" | "LBUI" => {
            let raw = memory.read_u8_checked(addr).context("faulting LBU")?;
            LoadResult {
                value: raw as u64,
                raw: raw as u64,
                size: 1,
            }
        }
        "LH" | "LHI" => {
            let raw = memory.read_u16_checked(addr).context("faulting LH")?;
            LoadResult {
                value: sign_extend(raw as u64, 16) as u64,
                raw: raw as u64,
                size: 2,
            }
        }
        "LHU" | "LHUI" => {
            let raw = memory.read_u16_checked(addr).context("faulting LHU")?;
            LoadResult {
                value: raw as u64,
                raw: raw as u64,
                size: 2,
            }
        }
        "LW" | "LWI" => {
            let raw = memory.read_u32_checked(addr).context("faulting LW")?;
            LoadResult {
                value: sign_extend(raw as u64, 32) as u64,
                raw: raw as u64,
                size: 4,
            }
        }
        "LWU" | "LWUI" => {
            let raw = memory.read_u32_checked(addr).context("faulting LWU")?;
            LoadResult {
                value: raw as u64,
                raw: raw as u64,
                size: 4,
            }
        }
        "LD" | "LDI" => {
            let raw = memory.read_u64_checked(addr).context("faulting LD")?;
            LoadResult {
                value: raw,
                raw,
                size: 8,
            }
        }
        other => bail!("unsupported load mnemonic {other}"),
    };
    Ok(result)
}

fn store_value(memory: &mut GuestMemory, mnemonic: &str, addr: u64, value: u64) -> Result<()> {
    match mnemonic {
        "SB" | "SBI" => memory
            .write_bytes_checked(addr, &[value as u8])
            .context("failed SB guest store")?,
        "SH" | "SHI" => memory
            .write_u16_checked(addr, value as u16)
            .context("failed SH guest store")?,
        "SW" | "SWI" => memory
            .write_u32_checked(addr, value as u32)
            .context("failed SW guest store")?,
        _ => memory
            .write_u64_checked(addr, value)
            .context("failed SD guest store")?,
    }
    Ok(())
}

fn pair_access(mnemonic: &str) -> Result<PairAccess> {
    let (load_mnemonic, store_mnemonic, elem_size) = match mnemonic {
        "HL.LBIP" | "HL.SBIP" => ("LB", "SB", 1),
        "HL.LBUIP" => ("LBU", "SB", 1),
        "HL.LHIP" | "HL.LHIP.U" | "HL.SHIP" | "HL.SHIP.U" => ("LH", "SH", 2),
        "HL.LHUIP" | "HL.LHUIP.U" => ("LHU", "SH", 2),
        "HL.LWIP" | "HL.LWIP.U" | "HL.SWIP" | "HL.SWIP.U" => ("LW", "SW", 4),
        "HL.LWUIP" | "HL.LWUIP.U" => ("LWU", "SW", 4),
        "HL.LDIP" | "HL.LDIP.U" | "HL.SDIP" | "HL.SDIP.U" => ("LD", "SD", 8),
        other => bail!("unsupported pair mnemonic {other}"),
    };
    Ok(PairAccess {
        load_mnemonic,
        store_mnemonic,
        elem_size,
        unscaled: mnemonic.ends_with(".U"),
    })
}

fn size_for_store(mnemonic: &str) -> u8 {
    match mnemonic {
        "SB" | "SBI" => 1,
        "SH" | "SHI" => 2,
        "SW" | "SWI" => 4,
        _ => 8,
    }
}

fn size_shift_for_store(mnemonic: &str) -> u8 {
    match mnemonic {
        "SB" => 0,
        "SH" => 1,
        "SW" => 2,
        _ => 3,
    }
}

fn apply_src_r_addsub(value: u64, src_type: u8, shamt: u32) -> u64 {
    let mut out = match src_type & 0x3 {
        0 => sign_extend32(value),
        1 => value as u32 as u64,
        2 => value.wrapping_neg(),
        _ => value,
    };
    if shamt != 0 {
        out = out.wrapping_shl(shamt & 0x3f);
    }
    out
}

fn apply_src_r_logic(value: u64, src_type: u8, shamt: u32) -> u64 {
    let mut out = match src_type & 0x3 {
        0 => sign_extend32(value),
        1 => value as u32 as u64,
        2 => !value,
        _ => value,
    };
    if shamt != 0 {
        out = out.wrapping_shl(shamt & 0x3f);
    }
    out
}

fn sign_extend(value: u64, width: u8) -> i64 {
    if width == 0 {
        return 0;
    }
    let shift = 64 - width;
    ((value << shift) as i64) >> shift
}

fn sign_extend32(value: u64) -> u64 {
    (value as u32 as i32 as i64) as u64
}

fn unsigned_div(lhs: u64, rhs: u64) -> u64 {
    if rhs == 0 { u64::MAX } else { lhs / rhs }
}

fn unsigned_rem(lhs: u64, rhs: u64) -> u64 {
    if rhs == 0 { lhs } else { lhs % rhs }
}

fn signed_div(lhs: u64, rhs: u64) -> u64 {
    let lhs = lhs as i64;
    let rhs = rhs as i64;
    if rhs == 0 {
        (-1i64) as u64
    } else if lhs == i64::MIN && rhs == -1 {
        lhs as u64
    } else {
        (lhs / rhs) as u64
    }
}

fn signed_rem(lhs: u64, rhs: u64) -> u64 {
    let lhs = lhs as i64;
    let rhs = rhs as i64;
    if rhs == 0 {
        lhs as u64
    } else if lhs == i64::MIN && rhs == -1 {
        0
    } else {
        (lhs % rhs) as u64
    }
}

fn resolve_open_path(runtime: &GuestRuntime, dirfd: i32, path: &str) -> String {
    if PathBuf::from(path).is_absolute() {
        return path.to_string();
    }
    if dirfd == GUEST_AT_FDCWD {
        if let Some(workdir) = &runtime.config.workdir {
            return workdir.join(path).display().to_string();
        }
    }
    path.to_string()
}

fn host_fstat(host_fd: i32) -> std::result::Result<GuestLinuxStat, i32> {
    let mut stat = unsafe { std::mem::zeroed::<libc::stat>() };
    let rc = unsafe { libc::fstat(host_fd, &mut stat as *mut libc::stat) };
    if rc != 0 {
        return Err(last_errno());
    }
    Ok(guest_linux_stat_from_host(&stat))
}

fn host_fstatat(
    state: &ExecState,
    runtime: &GuestRuntime,
    dirfd: i32,
    path: &str,
    flags: i32,
) -> std::result::Result<GuestLinuxStat, i32> {
    let resolved = resolve_open_path(runtime, dirfd, path);
    let c_path = CString::new(resolved.as_str()).map_err(|_| GUEST_EINVAL)?;
    let host_flags = if flags & GUEST_AT_SYMLINK_NOFOLLOW != 0 {
        libc::AT_SYMLINK_NOFOLLOW
    } else {
        0
    };
    let mut stat = unsafe { std::mem::zeroed::<libc::stat>() };
    let rc = if dirfd == GUEST_AT_FDCWD || resolved != path {
        unsafe {
            libc::fstatat(
                libc::AT_FDCWD,
                c_path.as_ptr(),
                &mut stat as *mut libc::stat,
                host_flags,
            )
        }
    } else {
        let host_dirfd = state.host_fd(dirfd as u64)?;
        unsafe {
            libc::fstatat(
                host_dirfd,
                c_path.as_ptr(),
                &mut stat as *mut libc::stat,
                host_flags,
            )
        }
    };
    if rc != 0 {
        return Err(last_errno());
    }
    Ok(guest_linux_stat_from_host(&stat))
}

fn guest_linux_stat_from_host(stat: &libc::stat) -> GuestLinuxStat {
    GuestLinuxStat {
        dev: stat.st_dev as u64,
        ino: stat.st_ino as u64,
        mode: stat.st_mode as u32,
        nlink: stat.st_nlink as u32,
        uid: stat.st_uid,
        gid: stat.st_gid,
        rdev: stat.st_rdev as u64,
        size: stat.st_size as i64,
        blksize: stat.st_blksize as i32,
        blocks: stat.st_blocks as i64,
        atime_sec: stat.st_atime as i64,
        atime_nsec: stat.st_atime_nsec as u64,
        mtime_sec: stat.st_mtime as i64,
        mtime_nsec: stat.st_mtime_nsec as u64,
        ctime_sec: stat.st_ctime as i64,
        ctime_nsec: stat.st_ctime_nsec as u64,
    }
}

fn write_guest_linux_stat(memory: &mut GuestMemory, addr: u64, stat: GuestLinuxStat) -> Result<()> {
    let mut bytes = [0u8; GUEST_LINUX_STAT_SIZE];
    write_u64_field(&mut bytes, 0, stat.dev);
    write_u64_field(&mut bytes, 8, stat.ino);
    write_u32_field(&mut bytes, 16, stat.mode);
    write_u32_field(&mut bytes, 20, stat.nlink);
    write_u32_field(&mut bytes, 24, stat.uid);
    write_u32_field(&mut bytes, 28, stat.gid);
    write_u64_field(&mut bytes, 32, stat.rdev);
    write_i64_field(&mut bytes, 48, stat.size);
    write_i32_field(&mut bytes, 56, stat.blksize);
    write_i64_field(&mut bytes, 64, stat.blocks);
    write_i64_field(&mut bytes, 72, stat.atime_sec);
    write_u64_field(&mut bytes, 80, stat.atime_nsec);
    write_i64_field(&mut bytes, 88, stat.mtime_sec);
    write_u64_field(&mut bytes, 96, stat.mtime_nsec);
    write_i64_field(&mut bytes, 104, stat.ctime_sec);
    write_u64_field(&mut bytes, 112, stat.ctime_nsec);
    memory
        .write_bytes_checked(addr, &bytes)
        .context("failed to write guest fstat buffer")
}

fn write_guest_utsname(memory: &mut GuestMemory, addr: u64) -> Result<()> {
    let mut bytes = [0u8; GUEST_UTSNAME_SIZE];
    write_guest_uts_field(&mut bytes, 0, "Linux")?;
    write_guest_uts_field(&mut bytes, 1, "linxcoremodel")?;
    write_guest_uts_field(&mut bytes, 2, "6.8.0-linx")?;
    write_guest_uts_field(&mut bytes, 3, "LinxCoreModel")?;
    write_guest_uts_field(&mut bytes, 4, "linx64")?;
    write_guest_uts_field(&mut bytes, 5, "localdomain")?;
    memory
        .write_bytes_checked(addr, &bytes)
        .context("failed to write guest utsname buffer")
}

fn write_guest_uts_field(buf: &mut [u8], index: usize, text: &str) -> Result<()> {
    let field = buf
        .get_mut(index * GUEST_UTS_FIELD_BYTES..(index + 1) * GUEST_UTS_FIELD_BYTES)
        .context("invalid utsname field index")?;
    if text.len() >= GUEST_UTS_FIELD_BYTES {
        bail!("utsname field is too long");
    }
    field[..text.len()].copy_from_slice(text.as_bytes());
    Ok(())
}

fn write_guest_sysinfo(memory: &mut GuestMemory, addr: u64, runtime: &GuestRuntime) -> Result<()> {
    let totalram = runtime.config.mem_bytes;
    let mapped = runtime
        .memory
        .regions
        .iter()
        .map(|region| region.size)
        .sum::<u64>();
    let bufferram = totalram / 16;
    let freeram = totalram.saturating_sub(mapped.min(totalram));
    let mut bytes = [0u8; GUEST_SYSINFO_SIZE];

    write_u64_field(&mut bytes, 0, 1);
    write_u64_field(&mut bytes, 8, 0);
    write_u64_field(&mut bytes, 16, 0);
    write_u64_field(&mut bytes, 24, 0);
    write_u64_field(&mut bytes, 32, totalram);
    write_u64_field(&mut bytes, 40, freeram);
    write_u64_field(&mut bytes, 48, 0);
    write_u64_field(&mut bytes, 56, bufferram);
    write_u64_field(&mut bytes, 64, 0);
    write_u64_field(&mut bytes, 72, 0);
    write_u16_field(&mut bytes, 80, 1);
    write_u16_field(&mut bytes, 82, 0);
    write_u64_field(&mut bytes, 88, 0);
    write_u64_field(&mut bytes, 96, 0);
    write_u32_field(&mut bytes, 104, 1);

    memory
        .write_bytes_checked(addr, &bytes)
        .context("failed to write guest sysinfo buffer")
}

fn dispatch_futex(state: &mut ExecState, args: [u64; 6]) -> std::result::Result<u64, i32> {
    let addr = args[0];
    let op = args[1] as i32;
    let val = args[2] as u32;
    let cmd = op & !(FUTEX_PRIVATE | FUTEX_CLOCK_REALTIME);

    match cmd {
        FUTEX_WAIT => {
            let Some(current) = state.memory.read_u32_checked(addr) else {
                return Err(GUEST_EFAULT);
            };
            if current != val {
                Err(GUEST_EAGAIN)
            } else if args[3] != 0 {
                Err(GUEST_ETIMEDOUT)
            } else {
                // Single-process mode cannot block on a real waiter queue yet.
                Err(GUEST_EAGAIN)
            }
        }
        FUTEX_WAKE => Ok(0),
        _ => Err(GUEST_ENOSYS),
    }
}

fn dispatch_getcwd(
    state: &mut ExecState,
    runtime: &GuestRuntime,
    args: [u64; 6],
) -> std::result::Result<u64, i32> {
    let buf = args[0];
    let size = args[1] as usize;
    if size == 0 {
        return Err(GUEST_EINVAL);
    }
    let cwd = runtime
        .config
        .workdir
        .clone()
        .or_else(|| std::env::current_dir().ok())
        .ok_or(GUEST_ENOENT)?;
    let cwd = cwd.to_string_lossy();
    if cwd.len() + 1 > size {
        return Err(GUEST_ERANGE);
    }
    let mut bytes = cwd.as_bytes().to_vec();
    bytes.push(0);
    if state.memory.write_bytes_checked(buf, &bytes).is_none() {
        return Err(GUEST_EFAULT);
    }
    Ok(buf)
}

fn dispatch_wait4(state: &mut ExecState, args: [u64; 6]) -> std::result::Result<u64, i32> {
    let _pid = args[0] as i64;
    let _options = args[2] as i32;
    if args[1] != 0 && state.memory.read_u32_checked(args[1]).is_none() {
        return Err(GUEST_EFAULT);
    }
    if args[3] != 0
        && state
            .memory
            .read_bytes_checked(args[3], GUEST_RUSAGE_SIZE)
            .is_none()
    {
        return Err(GUEST_EFAULT);
    }
    Err(GUEST_ECHILD)
}

fn set_nonblocking(fd: i32) -> std::result::Result<(), i32> {
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 {
        return Err(last_errno());
    }
    if unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
        return Err(last_errno());
    }
    Ok(())
}

fn create_pollable_pipe() -> std::result::Result<(i32, i32), i32> {
    let mut pipefd = [0i32; 2];
    if unsafe { libc::pipe(pipefd.as_mut_ptr()) } < 0 {
        return Err(last_errno());
    }
    if let Err(errno) = set_nonblocking(pipefd[0]).and_then(|_| set_nonblocking(pipefd[1])) {
        close_host_fd(pipefd[0]);
        close_host_fd(pipefd[1]);
        return Err(errno);
    }
    Ok((pipefd[0], pipefd[1]))
}

fn close_host_fd(fd: i32) {
    unsafe {
        libc::close(fd);
    }
}

fn guest_epoll_to_poll_events(events: u32) -> i16 {
    let mut poll_events = 0i16;
    if events & (GUEST_EPOLLIN | GUEST_EPOLLRDNORM) != 0 {
        poll_events |= libc::POLLIN;
    }
    if events & (GUEST_EPOLLPRI | GUEST_EPOLLRDBAND) != 0 {
        poll_events |= libc::POLLPRI;
    }
    if events & (GUEST_EPOLLOUT | GUEST_EPOLLWRNORM | GUEST_EPOLLWRBAND) != 0 {
        poll_events |= libc::POLLOUT;
    }
    poll_events
}

fn poll_revents_to_guest_epoll(revents: i16) -> u32 {
    let mut events = 0u32;
    if revents & libc::POLLIN != 0 {
        events |= GUEST_EPOLLIN | GUEST_EPOLLRDNORM;
    }
    if revents & libc::POLLPRI != 0 {
        events |= GUEST_EPOLLPRI | GUEST_EPOLLRDBAND;
    }
    if revents & libc::POLLOUT != 0 {
        events |= GUEST_EPOLLOUT | GUEST_EPOLLWRNORM;
    }
    if revents & libc::POLLERR != 0 {
        events |= GUEST_EPOLLERR;
    }
    if revents & libc::POLLHUP != 0 {
        events |= GUEST_EPOLLHUP | GUEST_EPOLLRDHUP;
    }
    if revents & libc::POLLNVAL != 0 {
        events |= GUEST_EPOLLNVAL;
    }
    #[cfg(any(target_os = "linux", target_os = "android"))]
    if revents & libc::POLLRDHUP != 0 {
        events |= GUEST_EPOLLRDHUP;
    }
    events
}

fn read_guest_epoll_event(memory: &GuestMemory, addr: u64) -> std::result::Result<(u32, u64), i32> {
    let events = memory.read_u32_checked(addr).ok_or(GUEST_EFAULT)?;
    let data = memory.read_u64_checked(addr + 8).ok_or(GUEST_EFAULT)?;
    Ok((events, data))
}

fn write_guest_epoll_event(
    memory: &mut GuestMemory,
    addr: u64,
    events: u32,
    data: u64,
) -> std::result::Result<(), i32> {
    if memory.write_u32_checked(addr, events).is_none()
        || memory.write_u32_checked(addr + 4, 0).is_none()
        || memory.write_u64_checked(addr + 8, data).is_none()
    {
        return Err(GUEST_EFAULT);
    }
    Ok(())
}

fn dispatch_eventfd2(state: &mut ExecState, args: [u64; 6]) -> std::result::Result<u64, i32> {
    let init = args[0];
    let flags = args[1] as i32;
    if flags & !(GUEST_EFD_SEMAPHORE | GUEST_O_NONBLOCK | GUEST_O_CLOEXEC) != 0 {
        return Err(GUEST_EINVAL);
    }
    let (read_fd, write_fd) = create_pollable_pipe()?;
    if init != 0 {
        let signal = [1u8; 1];
        if unsafe { libc::write(write_fd, signal.as_ptr().cast(), signal.len()) } < 0 {
            close_host_fd(read_fd);
            close_host_fd(write_fd);
            return Err(last_errno());
        }
    }
    let guest_fd = state.alloc_guest_fd();
    let fd_flags = if flags & GUEST_O_CLOEXEC != 0 {
        GUEST_FD_CLOEXEC
    } else {
        0
    };
    state.insert_guest_fd(guest_fd, read_fd, flags & !GUEST_O_CLOEXEC, fd_flags);
    state.special_fds.insert(
        guest_fd,
        SpecialFdKind::EventFd(EventFdState {
            write_fd,
            counter: init,
            semaphore: (flags & GUEST_EFD_SEMAPHORE) != 0,
        }),
    );
    Ok(guest_fd)
}

fn dispatch_eventfd_write(
    memory: &mut GuestMemory,
    eventfd: &mut EventFdState,
    args: [u64; 6],
) -> std::result::Result<u64, i32> {
    if args[2] != 8 {
        return Err(GUEST_EINVAL);
    }
    let value = memory.read_u64_checked(args[1]).ok_or(GUEST_EFAULT)?;
    if value == u64::MAX {
        return Err(GUEST_EINVAL);
    }
    if eventfd.counter > u64::MAX - 1 - value {
        return Err(GUEST_EAGAIN);
    }
    let was_zero = eventfd.counter == 0;
    eventfd.counter = eventfd.counter.saturating_add(value);
    if was_zero && eventfd.counter != 0 {
        let signal = [1u8; 1];
        if unsafe { libc::write(eventfd.write_fd, signal.as_ptr().cast(), signal.len()) } < 0 {
            let errno = last_errno();
            if errno != GUEST_EAGAIN {
                return Err(errno);
            }
        }
    }
    Ok(8)
}

fn dispatch_eventfd_read(
    memory: &mut GuestMemory,
    read_fd: i32,
    eventfd: &mut EventFdState,
    commit: &mut CommitRecord,
    args: [u64; 6],
) -> std::result::Result<u64, i32> {
    if args[2] != 8 {
        return Err(GUEST_EINVAL);
    }
    if eventfd.counter == 0 {
        return Err(GUEST_EAGAIN);
    }
    let value = if eventfd.semaphore {
        1
    } else {
        eventfd.counter
    };
    eventfd.counter -= value;
    if eventfd.counter == 0 {
        let mut byte = [0u8; 1];
        unsafe {
            libc::read(read_fd, byte.as_mut_ptr().cast(), byte.len());
        };
    }
    if memory.write_u64_checked(args[1], value).is_none() {
        return Err(GUEST_EFAULT);
    }
    record_store(commit, args[1], value, 8);
    Ok(8)
}

fn dispatch_epoll_create1(state: &mut ExecState, args: [u64; 6]) -> std::result::Result<u64, i32> {
    let flags = args[0] as i32;
    if flags & !(GUEST_O_CLOEXEC | GUEST_O_NONBLOCK) != 0 {
        return Err(GUEST_EINVAL);
    }
    let (read_fd, write_fd) = create_pollable_pipe()?;
    let guest_fd = state.alloc_guest_fd();
    let fd_flags = if flags & GUEST_O_CLOEXEC != 0 {
        GUEST_FD_CLOEXEC
    } else {
        0
    };
    state.insert_guest_fd(guest_fd, read_fd, flags & !GUEST_O_CLOEXEC, fd_flags);
    state.special_fds.insert(
        guest_fd,
        SpecialFdKind::Epoll(EpollState {
            wake_write_fd: write_fd,
            registrations: BTreeMap::new(),
        }),
    );
    Ok(guest_fd)
}

fn dispatch_epoll_ctl(state: &mut ExecState, args: [u64; 6]) -> std::result::Result<u64, i32> {
    let epfd = args[0];
    let op = args[1] as i32;
    let target_fd = args[2];
    let event_ptr = args[3];
    if target_fd == epfd {
        return Err(GUEST_EINVAL);
    }
    if matches!(
        state.special_fds.get(&target_fd),
        Some(SpecialFdKind::Epoll(_))
    ) {
        return Err(GUEST_EINVAL);
    }
    state.host_fd(target_fd)?;
    let registration = match op {
        GUEST_EPOLL_CTL_ADD | GUEST_EPOLL_CTL_MOD => {
            if event_ptr == 0 {
                return Err(GUEST_EFAULT);
            }
            let (events, data) = read_guest_epoll_event(&state.memory, event_ptr)?;
            Some(GuestEpollRegistration {
                guest_fd: target_fd,
                events,
                data,
            })
        }
        GUEST_EPOLL_CTL_DEL => None,
        _ => return Err(GUEST_EINVAL),
    };
    let Some(SpecialFdKind::Epoll(epoll)) = state.special_fds.get_mut(&epfd) else {
        return Err(GUEST_EBADF);
    };
    match op {
        GUEST_EPOLL_CTL_ADD => {
            if epoll.registrations.contains_key(&target_fd) {
                return Err(GUEST_EEXIST);
            }
            epoll.registrations.insert(target_fd, registration.unwrap());
        }
        GUEST_EPOLL_CTL_MOD => {
            if !epoll.registrations.contains_key(&target_fd) {
                return Err(GUEST_ENOENT);
            }
            epoll.registrations.insert(target_fd, registration.unwrap());
        }
        GUEST_EPOLL_CTL_DEL => {
            if epoll.registrations.remove(&target_fd).is_none() {
                return Err(GUEST_ENOENT);
            }
        }
        _ => unreachable!(),
    }
    Ok(0)
}

fn dispatch_epoll_pwait(
    state: &mut ExecState,
    commit: &mut CommitRecord,
    args: [u64; 6],
) -> std::result::Result<u64, i32> {
    let epfd = args[0];
    let events_ptr = args[1];
    let maxevents = usize::try_from(args[2]).map_err(|_| GUEST_EINVAL)?;
    let timeout_ms = i32::try_from(args[3] as i64).map_err(|_| GUEST_EINVAL)?;
    let sigmask_ptr = args[4];
    let sigset_size = usize::try_from(args[5]).map_err(|_| GUEST_EINVAL)?;
    if maxevents == 0 {
        return Err(GUEST_EINVAL);
    }
    validate_guest_sigmask(&state.memory, sigmask_ptr, sigset_size)?;

    let registrations = match state.special_fds.get(&epfd) {
        Some(SpecialFdKind::Epoll(epoll)) => {
            epoll.registrations.values().cloned().collect::<Vec<_>>()
        }
        _ => return Err(GUEST_EBADF),
    };
    let mut pollfds = Vec::with_capacity(registrations.len());
    for registration in &registrations {
        let host_fd = state.host_fd(registration.guest_fd)?;
        pollfds.push(libc::pollfd {
            fd: host_fd,
            events: guest_epoll_to_poll_events(registration.events),
            revents: 0,
        });
    }
    let rc = unsafe {
        libc::poll(
            pollfds.as_mut_ptr(),
            pollfds.len() as libc::nfds_t,
            timeout_ms,
        )
    };
    if rc < 0 {
        return Err(last_errno());
    }

    let mut ready_count = 0usize;
    let mut oneshot_remove = Vec::new();
    for (registration, pollfd) in registrations.iter().zip(pollfds.iter()) {
        let revents = poll_revents_to_guest_epoll(pollfd.revents)
            | (registration.events & (GUEST_EPOLLET | GUEST_EPOLLONESHOT));
        if revents == 0 {
            continue;
        }
        if ready_count < maxevents {
            let addr = events_ptr + (ready_count as u64) * GUEST_EPOLL_EVENT_SIZE;
            write_guest_epoll_event(&mut state.memory, addr, revents, registration.data)?;
        }
        ready_count += 1;
        if registration.events & GUEST_EPOLLONESHOT != 0 {
            oneshot_remove.push(registration.guest_fd);
        }
    }

    if let Some(SpecialFdKind::Epoll(epoll)) = state.special_fds.get_mut(&epfd) {
        for guest_fd in oneshot_remove {
            epoll.registrations.remove(&guest_fd);
        }
    }

    let produced = ready_count.min(maxevents);
    if produced != 0 {
        record_store(
            commit,
            events_ptr,
            produced as u64,
            trace_size(produced * GUEST_EPOLL_EVENT_SIZE as usize),
        );
    }
    Ok(produced as u64)
}

fn parse_guest_timeout_ms(memory: &GuestMemory, timeout_ptr: u64) -> std::result::Result<i32, i32> {
    if timeout_ptr == 0 {
        return Ok(-1);
    }
    let sec = memory.read_u64_checked(timeout_ptr).ok_or(GUEST_EFAULT)?;
    let nsec = memory
        .read_u64_checked(timeout_ptr + 8)
        .ok_or(GUEST_EFAULT)?;
    let sec = i64::try_from(sec).map_err(|_| GUEST_EINVAL)?;
    let nsec = i64::try_from(nsec).map_err(|_| GUEST_EINVAL)?;
    if sec < 0 || !(0..1_000_000_000).contains(&nsec) {
        return Err(GUEST_EINVAL);
    }
    let millis = sec
        .saturating_mul(1000)
        .saturating_add((nsec + 999_999) / 1_000_000);
    Ok(millis.min(i32::MAX as i64) as i32)
}

fn validate_guest_sigmask(
    memory: &GuestMemory,
    sigmask_ptr: u64,
    sigset_size: usize,
) -> std::result::Result<(), i32> {
    if sigmask_ptr == 0 {
        return Ok(());
    }
    if sigset_size == 0 || sigset_size > GUEST_SIGSET_MAX_BYTES {
        return Err(GUEST_EINVAL);
    }
    if memory
        .read_bytes_checked(sigmask_ptr, sigset_size)
        .is_none()
    {
        return Err(GUEST_EFAULT);
    }
    Ok(())
}

fn read_guest_fd_set(
    memory: &GuestMemory,
    addr: u64,
    nfds: usize,
) -> std::result::Result<Vec<bool>, i32> {
    let bytes = memory
        .read_bytes_checked(addr, GUEST_FD_SET_SIZE)
        .ok_or(GUEST_EFAULT)?;
    let mut set = vec![false; nfds];
    for fd in 0..nfds {
        let word = fd / 64;
        let bit = fd % 64;
        let mut raw = [0u8; 8];
        raw.copy_from_slice(&bytes[word * 8..word * 8 + 8]);
        let bits = u64::from_le_bytes(raw);
        set[fd] = ((bits >> bit) & 1) != 0;
    }
    Ok(set)
}

fn write_guest_fd_set(
    memory: &mut GuestMemory,
    addr: u64,
    ready: &[bool],
) -> std::result::Result<(), i32> {
    let mut bytes = [0u8; GUEST_FD_SET_SIZE];
    for (fd, is_set) in ready.iter().copied().enumerate() {
        if !is_set {
            continue;
        }
        let word = fd / 64;
        let bit = fd % 64;
        let range = word * 8..word * 8 + 8;
        let mut raw = [0u8; 8];
        raw.copy_from_slice(&bytes[range.clone()]);
        let value = u64::from_le_bytes(raw) | (1u64 << bit);
        bytes[range].copy_from_slice(&value.to_le_bytes());
    }
    memory
        .write_bytes_checked(addr, &bytes)
        .ok_or(GUEST_EFAULT)?;
    Ok(())
}

fn dispatch_pselect6(
    state: &mut ExecState,
    commit: &mut CommitRecord,
    args: [u64; 6],
) -> std::result::Result<u64, i32> {
    let nfds = usize::try_from(args[0]).map_err(|_| GUEST_EINVAL)?;
    if nfds > GUEST_FD_SET_SIZE * 8 {
        return Err(GUEST_EINVAL);
    }
    let timeout_ms = parse_guest_timeout_ms(&state.memory, args[4])?;

    if args[5] != 0 {
        let sigmask_ptr = state.memory.read_u64_checked(args[5]).ok_or(GUEST_EFAULT)?;
        let sigset_size = state
            .memory
            .read_u64_checked(args[5] + 8)
            .ok_or(GUEST_EFAULT)?;
        let sigset_size = usize::try_from(sigset_size).map_err(|_| GUEST_EINVAL)?;
        validate_guest_sigmask(&state.memory, sigmask_ptr, sigset_size)?;
    }

    let read_req = if args[1] != 0 {
        Some(read_guest_fd_set(&state.memory, args[1], nfds)?)
    } else {
        None
    };
    let write_req = if args[2] != 0 {
        Some(read_guest_fd_set(&state.memory, args[2], nfds)?)
    } else {
        None
    };
    let except_req = if args[3] != 0 {
        Some(read_guest_fd_set(&state.memory, args[3], nfds)?)
    } else {
        None
    };

    let mut pollfds = Vec::new();
    for fd in 0..nfds {
        let wants_read = read_req.as_ref().map(|set| set[fd]).unwrap_or(false);
        let wants_write = write_req.as_ref().map(|set| set[fd]).unwrap_or(false);
        let wants_except = except_req.as_ref().map(|set| set[fd]).unwrap_or(false);
        if !(wants_read || wants_write || wants_except) {
            continue;
        }
        let host_fd = state.host_fd(fd as u64)?;
        let mut events = 0i16;
        if wants_read {
            events |= libc::POLLIN;
        }
        if wants_write {
            events |= libc::POLLOUT;
        }
        if wants_except {
            events |= libc::POLLPRI;
        }
        pollfds.push((
            fd,
            libc::pollfd {
                fd: host_fd,
                events,
                revents: 0,
            },
        ));
    }

    let mut host_only: Vec<libc::pollfd> = pollfds.iter().map(|(_, pfd)| *pfd).collect();
    let rc = unsafe {
        libc::poll(
            host_only.as_mut_ptr(),
            host_only.len() as libc::nfds_t,
            timeout_ms,
        )
    };
    if rc < 0 {
        return Err(last_errno());
    }

    let mut read_ready = vec![false; nfds];
    let mut write_ready = vec![false; nfds];
    let mut except_ready = vec![false; nfds];
    let mut ready_count = 0u64;
    for ((fd, _), polled) in pollfds.iter().zip(host_only.iter()) {
        let mut any = false;
        if polled.revents & (libc::POLLIN | libc::POLLHUP | libc::POLLERR) != 0 {
            read_ready[*fd] = true;
            any = true;
        }
        if polled.revents & libc::POLLOUT != 0 {
            write_ready[*fd] = true;
            any = true;
        }
        if polled.revents & (libc::POLLPRI | libc::POLLERR) != 0 {
            except_ready[*fd] = true;
            any = true;
        }
        if any {
            ready_count += 1;
        }
    }

    if let Some(_) = read_req {
        write_guest_fd_set(&mut state.memory, args[1], &read_ready)?;
    }
    if let Some(_) = write_req {
        write_guest_fd_set(&mut state.memory, args[2], &write_ready)?;
    }
    if let Some(_) = except_req {
        write_guest_fd_set(&mut state.memory, args[3], &except_ready)?;
    }

    let traced_bytes = (args[1] != 0) as usize * GUEST_FD_SET_SIZE
        + (args[2] != 0) as usize * GUEST_FD_SET_SIZE
        + (args[3] != 0) as usize * GUEST_FD_SET_SIZE;
    if traced_bytes != 0 {
        let trace_addr = if args[1] != 0 {
            args[1]
        } else if args[2] != 0 {
            args[2]
        } else {
            args[3]
        };
        record_store(commit, trace_addr, ready_count, trace_size(traced_bytes));
    }
    Ok(ready_count)
}

fn dispatch_ppoll(
    state: &mut ExecState,
    commit: &mut CommitRecord,
    args: [u64; 6],
) -> std::result::Result<u64, i32> {
    let fds_addr = args[0];
    let nfds = usize::try_from(args[1]).map_err(|_| GUEST_EINVAL)?;
    let timeout_ptr = args[2];
    let sigmask_ptr = args[3];
    let sigset_size = usize::try_from(args[4]).map_err(|_| GUEST_EINVAL)?;

    validate_guest_sigmask(&state.memory, sigmask_ptr, sigset_size)?;
    let timeout_ms = parse_guest_timeout_ms(&state.memory, timeout_ptr)?;

    let mut host_fds = Vec::with_capacity(nfds);
    let mut forced_revents = vec![0u16; nfds];
    let mut forced_ready = 0u64;
    for idx in 0..nfds {
        let base = fds_addr + (idx as u64) * GUEST_POLLFD_SIZE;
        let guest_fd_raw = state.memory.read_u32_checked(base).ok_or(GUEST_EFAULT)?;
        let guest_fd = guest_fd_raw as i32;
        let events = state
            .memory
            .read_u16_checked(base + 4)
            .ok_or(GUEST_EFAULT)? as i16;
        let mut host_fd = -1i32;
        if guest_fd >= 0 {
            match state.host_fd(guest_fd as u64) {
                Ok(mapped) => host_fd = mapped,
                Err(_) => {
                    forced_revents[idx] = GUEST_POLLNVAL;
                    forced_ready += 1;
                }
            }
        }
        host_fds.push(libc::pollfd {
            fd: host_fd,
            events,
            revents: 0,
        });
    }

    let rc = unsafe {
        libc::poll(
            host_fds.as_mut_ptr(),
            host_fds.len() as libc::nfds_t,
            timeout_ms,
        )
    };
    if rc < 0 {
        return Err(last_errno());
    }

    let mut ready = forced_ready + rc as u64;
    for (idx, host_fd) in host_fds.iter().enumerate() {
        let base = fds_addr + (idx as u64) * GUEST_POLLFD_SIZE;
        let revents = if forced_revents[idx] != 0 {
            forced_revents[idx]
        } else {
            host_fd.revents as u16
        };
        if forced_revents[idx] == 0 && host_fd.fd < 0 && revents != 0 {
            ready += 1;
        }
        if state.memory.write_u16_checked(base + 6, revents).is_none() {
            return Err(GUEST_EFAULT);
        }
    }
    record_store(
        commit,
        fds_addr,
        ready,
        trace_size(nfds * GUEST_POLLFD_SIZE as usize),
    );
    Ok(ready)
}

fn dispatch_prctl(
    state: &mut ExecState,
    commit: &mut CommitRecord,
    args: [u64; 6],
) -> std::result::Result<u64, i32> {
    match args[0] {
        GUEST_PR_SET_NAME => {
            state.thread_name = read_guest_prctl_name(&state.memory, args[1])?;
            Ok(0)
        }
        GUEST_PR_GET_NAME => {
            if state
                .memory
                .write_bytes_checked(args[1], &state.thread_name)
                .is_none()
            {
                return Err(GUEST_EFAULT);
            }
            record_store(commit, args[1], 0, GUEST_PRCTL_NAME_BYTES as u8);
            Ok(0)
        }
        _ => Err(GUEST_ENOSYS),
    }
}

fn dispatch_getrandom(
    state: &mut ExecState,
    commit: &mut CommitRecord,
    args: [u64; 6],
) -> std::result::Result<u64, i32> {
    let buf = args[0];
    let len = args[1] as usize;
    let flags = args[2];
    if flags & !0x3 != 0 {
        return Err(GUEST_EINVAL);
    }
    let mut bytes = vec![0u8; len];
    for byte in &mut bytes {
        state.random_state ^= state.random_state << 13;
        state.random_state ^= state.random_state >> 7;
        state.random_state ^= state.random_state << 17;
        *byte = state.random_state as u8;
    }
    if state.memory.write_bytes_checked(buf, &bytes).is_none() {
        return Err(GUEST_EFAULT);
    }
    record_store(commit, buf, 0, trace_size(len));
    Ok(len as u64)
}

fn dispatch_membarrier(state: &mut ExecState, args: [u64; 6]) -> std::result::Result<u64, i32> {
    if args[1] != 0 {
        return Err(GUEST_EINVAL);
    }
    match args[0] {
        GUEST_MEMBARRIER_CMD_QUERY => Ok(GUEST_MEMBARRIER_CMD_PRIVATE_EXPEDITED
            | GUEST_MEMBARRIER_CMD_REGISTER_PRIVATE_EXPEDITED),
        GUEST_MEMBARRIER_CMD_REGISTER_PRIVATE_EXPEDITED => {
            state.membarrier_private_expedited = true;
            Ok(0)
        }
        GUEST_MEMBARRIER_CMD_PRIVATE_EXPEDITED => {
            if state.membarrier_private_expedited {
                Ok(0)
            } else {
                Err(GUEST_EPERM)
            }
        }
        _ => Err(GUEST_ENOSYS),
    }
}

fn dispatch_sigaltstack(
    state: &mut ExecState,
    commit: &mut CommitRecord,
    args: [u64; 6],
) -> std::result::Result<u64, i32> {
    let new_addr = args[0];
    let old_addr = args[1];

    if old_addr != 0 {
        write_guest_sigaltstack(
            &mut state.memory,
            old_addr,
            state.alt_stack_sp,
            state.alt_stack_flags,
            state.alt_stack_size,
        )?;
        record_store(
            commit,
            old_addr,
            state.alt_stack_sp,
            GUEST_SIGALTSTACK_SIZE as u8,
        );
    }

    if new_addr != 0 {
        let (sp, flags, size) = read_guest_sigaltstack(&state.memory, new_addr)?;
        if flags & GUEST_SS_ONSTACK != 0 || flags & !(GUEST_SS_DISABLE | GUEST_SS_ONSTACK) != 0 {
            return Err(GUEST_EINVAL);
        }
        if flags & GUEST_SS_DISABLE != 0 {
            state.alt_stack_sp = 0;
            state.alt_stack_size = 0;
            state.alt_stack_flags = GUEST_SS_DISABLE;
        } else {
            if sp == 0 {
                return Err(GUEST_EINVAL);
            }
            if size < GUEST_MINSIGSTKSZ {
                return Err(GUEST_ENOMEM);
            }
            state.alt_stack_sp = sp;
            state.alt_stack_size = size;
            state.alt_stack_flags = 0;
        }
    }
    Ok(0)
}

fn dispatch_rseq(
    state: &mut ExecState,
    commit: &mut CommitRecord,
    args: [u64; 6],
) -> std::result::Result<u64, i32> {
    let addr = args[0];
    let len = u32::try_from(args[1]).map_err(|_| GUEST_EINVAL)?;
    let flags = args[2];
    let sig = u32::try_from(args[3]).map_err(|_| GUEST_EINVAL)?;

    if flags == GUEST_RSEQ_FLAG_UNREGISTER {
        if state.rseq_addr == 0 || state.rseq_addr != addr {
            return Err(GUEST_EINVAL);
        }
        state.rseq_addr = 0;
        state.rseq_len = 0;
        state.rseq_sig = 0;
        return Ok(0);
    }
    if flags != 0 {
        return Err(GUEST_EINVAL);
    }
    if len < GUEST_RSEQ_MIN_LEN {
        return Err(GUEST_EINVAL);
    }
    if sig != GUEST_RSEQ_SIG {
        return Err(GUEST_EINVAL);
    }
    if addr == 0 {
        return Err(GUEST_EFAULT);
    }
    initialize_guest_rseq(&mut state.memory, addr, len, 0)?;
    state.rseq_addr = addr;
    state.rseq_len = len;
    state.rseq_sig = sig;
    record_store(commit, addr, 0, trace_size(len as usize));
    Ok(0)
}

fn dispatch_fcntl(state: &mut ExecState, args: [u64; 6]) -> std::result::Result<u64, i32> {
    let guest_fd = args[0];
    state.host_fd(guest_fd)?;
    let cmd = args[1] as i32;
    let value = args[2] as i32;
    match cmd {
        GUEST_F_DUPFD => {
            let min_fd = u64::try_from(value.max(0)).map_err(|_| GUEST_EINVAL)?;
            state.duplicate_guest_fd(guest_fd, min_fd, false)
        }
        GUEST_F_DUPFD_CLOEXEC => {
            let min_fd = u64::try_from(value.max(0)).map_err(|_| GUEST_EINVAL)?;
            state.duplicate_guest_fd(guest_fd, min_fd, true)
        }
        GUEST_F_GETFD => Ok(state.fd_fd_flags.get(&guest_fd).copied().unwrap_or(0) as u64),
        GUEST_F_SETFD => {
            state.fd_fd_flags.insert(guest_fd, value & GUEST_FD_CLOEXEC);
            Ok(0)
        }
        GUEST_F_GETFL => Ok(state.fd_status_flags.get(&guest_fd).copied().unwrap_or(0) as u64),
        GUEST_F_SETFL => {
            let entry = state.fd_status_flags.entry(guest_fd).or_insert(0);
            *entry = (*entry & 0b11) | (value & !0b11);
            Ok(0)
        }
        _ => Err(GUEST_ENOSYS),
    }
}

fn dispatch_ioctl(
    state: &mut ExecState,
    commit: &mut CommitRecord,
    args: [u64; 6],
) -> std::result::Result<u64, i32> {
    let guest_fd = args[0];
    state.host_fd(guest_fd)?;
    if guest_fd > 2 {
        return Err(GUEST_ENOTTY);
    }
    match args[1] {
        GUEST_TIOCGWINSZ => {
            let addr = args[2];
            if state.memory.write_u16_checked(addr, 24).is_none()
                || state.memory.write_u16_checked(addr + 2, 80).is_none()
                || state.memory.write_u16_checked(addr + 4, 0).is_none()
                || state.memory.write_u16_checked(addr + 6, 0).is_none()
            {
                return Err(GUEST_EFAULT);
            }
            record_store(commit, addr, 0, 8);
            Ok(0)
        }
        GUEST_TIOCGPGRP => {
            let addr = args[2];
            if state
                .memory
                .write_u32_checked(addr, state.current_pgrp)
                .is_none()
            {
                return Err(GUEST_EFAULT);
            }
            record_store(commit, addr, state.current_pgrp as u64, 4);
            Ok(0)
        }
        GUEST_TIOCSPGRP => {
            let addr = args[2];
            let Some(pgrp) = state.memory.read_u32_checked(addr) else {
                return Err(GUEST_EFAULT);
            };
            state.current_pgrp = pgrp;
            Ok(0)
        }
        _ => Err(GUEST_ENOTTY),
    }
}

fn dispatch_dup3(state: &mut ExecState, args: [u64; 6]) -> std::result::Result<u64, i32> {
    let old_guest_fd = args[0];
    let new_guest_fd = args[1];
    let flags = args[2] as i32;
    if flags & !GUEST_O_CLOEXEC != 0 {
        return Err(GUEST_EINVAL);
    }
    state.duplicate_guest_fd_to(old_guest_fd, new_guest_fd, (flags & GUEST_O_CLOEXEC) != 0)
}

fn dispatch_pipe2(
    state: &mut ExecState,
    commit: &mut CommitRecord,
    args: [u64; 6],
) -> std::result::Result<u64, i32> {
    let guest_pipefd = args[0];
    let flags = args[1] as i32;
    if flags & !(GUEST_O_CLOEXEC | GUEST_O_NONBLOCK) != 0 {
        return Err(GUEST_EINVAL);
    }

    let mut host_pipe = [-1i32; 2];
    let rc = unsafe { libc::pipe(host_pipe.as_mut_ptr()) };
    if rc != 0 {
        return Err(last_errno());
    }

    let read_guest_fd = state.alloc_guest_fd();
    let write_guest_fd = state.alloc_guest_fd_from(read_guest_fd + 1);
    if state
        .memory
        .write_u32_checked(guest_pipefd, read_guest_fd as u32)
        .is_none()
        || state
            .memory
            .write_u32_checked(guest_pipefd + 4, write_guest_fd as u32)
            .is_none()
    {
        unsafe {
            libc::close(host_pipe[0]);
            libc::close(host_pipe[1]);
        }
        return Err(GUEST_EFAULT);
    }

    let fd_flags = if flags & GUEST_O_CLOEXEC != 0 {
        GUEST_FD_CLOEXEC
    } else {
        0
    };
    let status_flags = flags & !GUEST_O_CLOEXEC;
    let read_status = status_flags & !GUEST_O_WRONLY;
    let write_status = (status_flags & !GUEST_O_RDONLY) | GUEST_O_WRONLY;
    state.insert_guest_fd(read_guest_fd, host_pipe[0], read_status, fd_flags);
    state.insert_guest_fd(write_guest_fd, host_pipe[1], write_status, fd_flags);
    record_store(commit, guest_pipefd, read_guest_fd, 8);
    Ok(0)
}

fn dispatch_madvise(state: &mut ExecState, args: [u64; 6]) -> std::result::Result<u64, i32> {
    let addr = args[0];
    let size = args[1];
    if size == 0 {
        return Ok(0);
    }
    if !state.memory.is_range_mapped(addr, size) {
        return Err(GUEST_ENOMEM);
    }
    match args[2] {
        0 | 1 | 2 | 3 | 4 | 8 | 9 | 10 | 11 | 12 | 13 | 14 | 15 | 16 | 17 | 18 | 19 | 20 | 21
        | 100 | 101 => Ok(0),
        _ => Err(GUEST_EINVAL),
    }
}

fn dispatch_prlimit64(state: &mut ExecState, args: [u64; 6]) -> std::result::Result<u64, i32> {
    let pid = args[0];
    let resource = args[1] as u32;
    let new_limit_ptr = args[2];
    let old_limit_ptr = args[3];

    if pid != 0 && pid != state.current_pid {
        return Err(GUEST_ESRCH);
    }

    let current = state.rlimit_for(resource).ok_or(GUEST_EINVAL)?;
    if old_limit_ptr != 0 {
        write_guest_rlimit(&mut state.memory, old_limit_ptr, current)?;
    }
    if new_limit_ptr != 0 {
        let new_limit = read_guest_rlimit(&state.memory, new_limit_ptr)?;
        state.set_rlimit(resource, new_limit)?;
    }
    Ok(0)
}

fn dispatch_newfstatat(
    state: &mut ExecState,
    runtime: &GuestRuntime,
    commit: &mut CommitRecord,
    args: [u64; 6],
) -> std::result::Result<u64, i32> {
    let dirfd = args[0] as i64 as i32;
    let stat_ptr = args[2];
    let flags = args[3] as i32;
    if flags & !(GUEST_AT_EMPTY_PATH | GUEST_AT_SYMLINK_NOFOLLOW) != 0 {
        return Err(GUEST_EINVAL);
    }
    let path = if args[1] == 0 {
        String::new()
    } else {
        state
            .memory
            .read_c_string_checked(args[1], MAX_C_STRING)
            .ok_or(GUEST_EFAULT)?
    };
    let stat = if path.is_empty() && (flags & GUEST_AT_EMPTY_PATH) != 0 {
        let host_fd = state.host_fd(args[0])?;
        host_fstat(host_fd)?
    } else {
        host_fstatat(state, runtime, dirfd, &path, flags)?
    };
    write_guest_linux_stat(&mut state.memory, stat_ptr, stat).map_err(|_| GUEST_EFAULT)?;
    record_store(
        commit,
        stat_ptr,
        stat.size as u64,
        GUEST_LINUX_STAT_SIZE as u8,
    );
    Ok(0)
}

fn dispatch_readlinkat(
    state: &mut ExecState,
    runtime: &GuestRuntime,
    commit: &mut CommitRecord,
    args: [u64; 6],
) -> std::result::Result<u64, i32> {
    let dirfd = args[0] as i64 as i32;
    let Some(path) = state.memory.read_c_string_checked(args[1], MAX_C_STRING) else {
        return Err(GUEST_EFAULT);
    };
    let resolved = resolve_open_path(runtime, dirfd, &path);
    let c_path = CString::new(resolved.as_str()).map_err(|_| GUEST_EINVAL)?;
    let mut bytes = vec![0u8; args[3] as usize];
    let rc = if dirfd == GUEST_AT_FDCWD || resolved != path {
        unsafe { libc::readlink(c_path.as_ptr(), bytes.as_mut_ptr().cast(), bytes.len()) }
    } else {
        let host_dirfd = state.host_fd(args[0])?;
        unsafe {
            libc::readlinkat(
                host_dirfd,
                c_path.as_ptr(),
                bytes.as_mut_ptr().cast(),
                bytes.len(),
            )
        }
    };
    if rc < 0 {
        return Err(last_errno());
    }
    let count = rc as usize;
    if state
        .memory
        .write_bytes_checked(args[2], &bytes[..count])
        .is_none()
    {
        return Err(GUEST_EFAULT);
    }
    record_store(commit, args[2], 0, trace_size(count));
    Ok(count as u64)
}

fn read_guest_sigaltstack(
    memory: &GuestMemory,
    addr: u64,
) -> std::result::Result<(u64, u32, u64), i32> {
    let sp = memory.read_u64_checked(addr).ok_or(GUEST_EFAULT)?;
    let flags = memory.read_u32_checked(addr + 8).ok_or(GUEST_EFAULT)?;
    let size = memory.read_u64_checked(addr + 16).ok_or(GUEST_EFAULT)?;
    Ok((sp, flags, size))
}

fn write_guest_sigaltstack(
    memory: &mut GuestMemory,
    addr: u64,
    sp: u64,
    flags: u32,
    size: u64,
) -> std::result::Result<(), i32> {
    if memory.write_u64_checked(addr, sp).is_none()
        || memory.write_u32_checked(addr + 8, flags).is_none()
        || memory.write_u32_checked(addr + 12, 0).is_none()
        || memory.write_u64_checked(addr + 16, size).is_none()
    {
        return Err(GUEST_EFAULT);
    }
    Ok(())
}

fn read_guest_prctl_name(
    memory: &GuestMemory,
    addr: u64,
) -> std::result::Result<[u8; GUEST_PRCTL_NAME_BYTES], i32> {
    let mut bytes = [0u8; GUEST_PRCTL_NAME_BYTES];
    for (idx, slot) in bytes.iter_mut().enumerate() {
        let value = memory
            .read_u8_checked(addr + idx as u64)
            .ok_or(GUEST_EFAULT)?;
        *slot = value;
        if value == 0 {
            break;
        }
    }
    if bytes[GUEST_PRCTL_NAME_BYTES - 1] != 0 {
        bytes[GUEST_PRCTL_NAME_BYTES - 1] = 0;
    }
    Ok(bytes)
}

fn initialize_guest_rseq(
    memory: &mut GuestMemory,
    addr: u64,
    len: u32,
    cpu_id: u32,
) -> std::result::Result<(), i32> {
    let zero = vec![0u8; len as usize];
    if memory.write_bytes_checked(addr, &zero).is_none() {
        return Err(GUEST_EFAULT);
    }
    if memory.write_u32_checked(addr, cpu_id).is_none()
        || memory.write_u32_checked(addr + 4, cpu_id).is_none()
        || memory.write_u64_checked(addr + 8, 0).is_none()
        || memory.write_u32_checked(addr + 16, 0).is_none()
    {
        return Err(GUEST_EFAULT);
    }
    Ok(())
}

fn dispatch_getres_ids(
    memory: &mut GuestMemory,
    addrs: [u64; 3],
    ids: [u32; 3],
) -> std::result::Result<(), i32> {
    for (addr, id) in addrs.into_iter().zip(ids) {
        if addr == 0 {
            continue;
        }
        if memory.write_u32_checked(addr, id).is_none() {
            return Err(GUEST_EFAULT);
        }
    }
    Ok(())
}

fn validate_single_id(raw: u64, current: &[u32; 3]) -> std::result::Result<u32, i32> {
    let value = raw.try_into().map_err(|_| GUEST_EINVAL)?;
    if value == 0 || current.contains(&value) {
        Ok(value)
    } else {
        Err(GUEST_EPERM)
    }
}

fn apply_setres_ids(current: [u32; 3], requested: [u64; 3]) -> std::result::Result<[u32; 3], i32> {
    let mut next = current;
    for (idx, raw) in requested.into_iter().enumerate() {
        if raw == u64::MAX {
            continue;
        }
        let value: u32 = raw.try_into().map_err(|_| GUEST_EINVAL)?;
        if value != 0 && !current.contains(&value) {
            return Err(GUEST_EPERM);
        }
        next[idx] = value;
    }
    Ok(next)
}

fn read_guest_rlimit(memory: &GuestMemory, addr: u64) -> std::result::Result<GuestRlimit, i32> {
    let Some(cur) = memory.read_u64_checked(addr) else {
        return Err(GUEST_EFAULT);
    };
    let Some(max) = memory.read_u64_checked(addr + 8) else {
        return Err(GUEST_EFAULT);
    };
    Ok(GuestRlimit { cur, max })
}

fn write_guest_rlimit(
    memory: &mut GuestMemory,
    addr: u64,
    limit: GuestRlimit,
) -> std::result::Result<(), i32> {
    if memory.write_u64_checked(addr, limit.cur).is_none()
        || memory.write_u64_checked(addr + 8, limit.max).is_none()
    {
        return Err(GUEST_EFAULT);
    }
    Ok(())
}

fn write_u32_field(bytes: &mut [u8], offset: usize, value: u32) {
    bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}

fn write_u16_field(bytes: &mut [u8], offset: usize, value: u16) {
    bytes[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
}

fn write_i32_field(bytes: &mut [u8], offset: usize, value: i32) {
    bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}

fn write_u64_field(bytes: &mut [u8], offset: usize, value: u64) {
    bytes[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
}

fn write_i64_field(bytes: &mut [u8], offset: usize, value: i64) {
    bytes[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
}

fn empty_commit(
    cycle: u64,
    pc: u64,
    decoded: &DecodedInstruction,
    runtime: &GuestRuntime,
) -> CommitRecord {
    let mut commit = CommitRecord::unsupported(
        cycle,
        pc,
        decoded.instruction_bits,
        TRAP_ILLEGAL_INST,
        &runtime.block,
    );
    commit.len = decoded.length_bytes();
    commit.trap_valid = 0;
    commit.trap_cause = 0;
    commit.traparg0 = 0;
    commit.next_pc = pc + decoded.length_bytes() as u64;
    commit
}

fn stage_event(cycle: u64, runtime: &GuestRuntime, stage: &str, cause: &str) -> StageTraceEvent {
    StageTraceEvent {
        cycle,
        row_id: format!("uop{cycle}"),
        stage_id: stage.to_string(),
        lane_id: runtime.block.lane_id.clone(),
        stall: false,
        cause: cause.to_string(),
        checkpoint_id: None,
        trap_cause: None,
        traparg0: None,
        target_setup_epoch: None,
        boundary_epoch: None,
        target_source_owner_row_id: None,
        target_source_epoch: None,
        target_owner_row_id: None,
        target_producer_kind: None,
        branch_kind: None,
        return_kind: None,
        call_materialization_kind: None,
        target_source_kind: None,
    }
}

fn reg_field(decoded: &DecodedInstruction, names: &[&str]) -> Result<usize> {
    field_u(decoded, names)
        .map(|value| value as usize)
        .with_context(|| format!("missing register field {:?} in {}", names, decoded.mnemonic))
}

fn field_u(decoded: &DecodedInstruction, names: &[&str]) -> Option<u64> {
    names
        .iter()
        .find_map(|name| decoded.field(name).map(|field| field.value_u64))
}

fn need_u(decoded: &DecodedInstruction, names: &[&str]) -> Result<u64> {
    field_u(decoded, names)
        .with_context(|| format!("missing unsigned field {:?} in {}", names, decoded.mnemonic))
}

fn field_i(decoded: &DecodedInstruction, names: &[&str]) -> Result<i64> {
    for name in names {
        if let Some(field) = decoded.field(name) {
            return Ok(field.value_i64.unwrap_or(field.value_u64 as i64));
        }
    }
    bail!("missing signed field {:?} in {}", names, decoded.mnemonic)
}

fn record_src0(commit: &mut CommitRecord, reg: usize, value: u64) {
    commit.src0_valid = 1;
    commit.src0_reg = reg as u8;
    commit.src0_data = value;
}

fn record_src1(commit: &mut CommitRecord, reg: usize, value: u64) {
    commit.src1_valid = 1;
    commit.src1_reg = reg as u8;
    commit.src1_data = value;
}

fn writeback(state: &mut ExecState, commit: &mut CommitRecord, reg: usize, value: u64) {
    let committed_reg = match reg {
        REG_IMPLICIT_T_DST => {
            state.push_t(value);
            REG_T1
        }
        REG_IMPLICIT_U_DST => {
            state.push_u(value);
            REG_U1
        }
        _ => {
            state.write_reg(reg, value);
            reg
        }
    };
    commit.dst_valid = 1;
    commit.dst_reg = committed_reg as u8;
    commit.dst_data = value;
    commit.wb_valid = 1;
    commit.wb_rd = committed_reg as u8;
    commit.wb_data = value;
}

fn record_load(commit: &mut CommitRecord, addr: u64, data: u64, size: u8) {
    commit.mem_valid = 1;
    commit.mem_is_store = 0;
    commit.mem_addr = addr;
    commit.mem_rdata = data;
    commit.mem_size = size;
}

fn record_store(commit: &mut CommitRecord, addr: u64, data: u64, size: u8) {
    commit.mem_valid = 1;
    commit.mem_is_store = 1;
    commit.mem_addr = addr;
    commit.mem_wdata = data;
    commit.mem_size = size;
}

fn trace_size(size: usize) -> u8 {
    size.min(u8::MAX as usize) as u8
}

fn align_up(value: u64, align: u64) -> u64 {
    debug_assert!(align.is_power_of_two());
    (value + align - 1) & !(align - 1)
}

fn align_down(value: u64, align: u64) -> u64 {
    debug_assert!(align.is_power_of_two());
    value & !(align - 1)
}

fn last_errno() -> i32 {
    let host_errno = std::io::Error::last_os_error()
        .raw_os_error()
        .unwrap_or(libc::EINVAL);
    host_errno_to_guest(host_errno)
}

fn host_errno_to_guest(host_errno: i32) -> i32 {
    match host_errno {
        x if x == libc::EPERM => GUEST_EPERM,
        x if x == libc::ENOENT => GUEST_ENOENT,
        x if x == libc::EAGAIN => GUEST_EAGAIN,
        x if x == libc::EBADF => GUEST_EBADF,
        x if x == libc::EFAULT => GUEST_EFAULT,
        x if x == libc::EINVAL => GUEST_EINVAL,
        x if x == libc::ENOTTY => GUEST_ENOTTY,
        x if x == libc::ENOMEM => GUEST_ENOMEM,
        x if x == libc::ERANGE => GUEST_ERANGE,
        x if x == libc::ENOSYS => GUEST_ENOSYS,
        x if x == libc::ETIMEDOUT => GUEST_ETIMEDOUT,
        _ => host_errno,
    }
}

fn finalize_syscall(
    state: &mut ExecState,
    commit: &mut CommitRecord,
    result: std::result::Result<u64, i32>,
) -> Result<Option<ExitSignal>> {
    let a0 = match result {
        Ok(value) => value,
        Err(errno) => (-(errno as i64)) as u64,
    };
    writeback(state, commit, REG_A0, a0);
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use elf::{LoadedElf, SegmentImage};
    use runtime::{BootInfo, MEM_READ, MEM_WRITE, RuntimeConfig};
    use std::fs;
    use std::io::Write;
    use std::os::fd::AsRawFd;
    use std::os::unix::fs::symlink;
    use tempfile::NamedTempFile;
    use tempfile::tempdir;

    #[test]
    fn executes_addi_and_exit_syscall() {
        let program = vec![
            enc_addi(REG_A0 as u32, REG_ZERO as u32, 7),
            enc_addi(REG_A7 as u32, REG_ZERO as u32, SYS_EXIT as u32),
            enc_acrc(1),
        ];
        let runtime = sample_runtime(&program, &[]);
        let bundle = FuncEngine
            .run(&runtime, &FuncRunOptions { max_steps: 16 })
            .unwrap();
        assert_eq!(bundle.result.metrics.exit_reason, "guest_exit(7)");
        assert_eq!(bundle.result.commits.len(), 3);
    }

    #[test]
    fn executes_memory_store_and_load() {
        let data_addr = 0x0800u32;
        let program = vec![
            enc_addi(20, REG_ZERO as u32, data_addr),
            enc_addi(21, REG_ZERO as u32, 11),
            enc_swi(21, 20, 0),
            enc_lwi(2, 20, 0),
            enc_addi(REG_A7 as u32, REG_ZERO as u32, SYS_EXIT as u32),
            enc_acrc(1),
        ];
        let data = vec![runtime::MemoryRegion {
            base: data_addr as u64,
            size: 0x1000,
            flags: 0b110,
            data: vec![0; 0x1000],
        }];
        let runtime = sample_runtime(&program, &data);
        let bundle = FuncEngine
            .run(&runtime, &FuncRunOptions { max_steps: 32 })
            .unwrap();
        assert_eq!(bundle.result.metrics.exit_reason, "guest_exit(11)");
    }

    #[test]
    fn implicit_t_destination_feeds_t1_consumers() {
        let program = vec![
            enc_addi(REG_IMPLICIT_T_DST as u32, REG_ZERO as u32, 5),
            enc_addi(REG_A0 as u32, REG_T1 as u32, 6),
            enc_addi(REG_A7 as u32, REG_ZERO as u32, SYS_EXIT as u32),
            enc_acrc(1),
        ];
        let runtime = sample_runtime(&program, &[]);
        let bundle = FuncEngine
            .run(&runtime, &FuncRunOptions { max_steps: 16 })
            .unwrap();
        assert_eq!(bundle.result.metrics.exit_reason, "guest_exit(11)");
        assert_eq!(bundle.result.commits[0].wb_rd as usize, REG_T1);
    }

    #[test]
    fn fstat_writes_linux_guest_layout() {
        let stat_addr = 0x4000u64;
        let data = vec![runtime::MemoryRegion {
            base: stat_addr,
            size: 0x1000,
            flags: 0b110,
            data: vec![0; 0x1000],
        }];
        let runtime = sample_runtime(&[enc_acrc(1)], &data);
        let mut state = ExecState::from_runtime(&runtime);
        let mut host_file = NamedTempFile::new().unwrap();
        host_file.write_all(b"stat-fixture\n").unwrap();
        host_file.flush().unwrap();

        let host_fd = unsafe { libc::dup(host_file.as_raw_fd()) };
        assert!(host_fd >= 0);

        let guest_fd = 9u64;
        state.fd_table.insert(guest_fd, host_fd);
        state.write_reg(REG_A7, SYS_FSTAT);
        state.write_reg(REG_A0, guest_fd);
        state.write_reg(REG_A1, stat_addr);
        let mut commit = CommitRecord::unsupported(0, 0, 0, TRAP_ILLEGAL_INST, &runtime.block);

        dispatch_syscall(&mut state, &runtime, &mut commit).unwrap();

        assert_eq!(state.read_reg(REG_A0), 0);
        assert_eq!(state.memory.read_u64(stat_addr + 48).unwrap(), 13);
        assert_eq!(
            state.memory.read_u32(stat_addr + 16).unwrap() & libc::S_IFMT as u32,
            libc::S_IFREG as u32
        );
        assert_eq!(state.memory.read_u64(stat_addr + 40).unwrap(), 0);
        assert_eq!(state.memory.read_u32(stat_addr + 124).unwrap(), 0);

        unsafe {
            libc::close(host_fd);
        }
    }

    #[test]
    fn brk_shrink_removes_unmapped_tail() {
        let runtime = sample_runtime(&[enc_acrc(1)], &[]);
        let mut state = ExecState::from_runtime(&runtime);
        let heap_base = state.brk_base;

        state.grow_brk(heap_base + PAGE_SIZE);
        assert_eq!(state.brk_current, heap_base + PAGE_SIZE);
        assert!(state.memory.read_u8(heap_base).is_some());

        state.grow_brk(heap_base);
        assert_eq!(state.brk_current, heap_base);
        assert!(state.memory.read_u8(heap_base).is_none());
        assert!(
            !state
                .memory
                .regions
                .iter()
                .any(|region| region.base == heap_base)
        );
    }

    #[test]
    fn mprotect_enforces_guest_write_buffer_permissions() {
        let data_addr = 0x4000u64;
        let data = vec![runtime::MemoryRegion {
            base: data_addr,
            size: PAGE_SIZE,
            flags: MEM_READ | MEM_WRITE,
            data: vec![b'X'; PAGE_SIZE as usize],
        }];
        let runtime = sample_runtime(&[enc_acrc(1)], &data);
        let mut state = ExecState::from_runtime(&runtime);
        let mut commit = CommitRecord::unsupported(0, 0, 0, TRAP_ILLEGAL_INST, &runtime.block);

        state.write_reg(REG_A7, SYS_MPROTECT);
        state.write_reg(REG_A0, data_addr);
        state.write_reg(REG_A1, PAGE_SIZE);
        state.write_reg(REG_A2, libc::PROT_NONE as u64);
        dispatch_syscall(&mut state, &runtime, &mut commit).unwrap();
        assert_eq!(state.read_reg(REG_A0), 0);

        state.write_reg(REG_A7, SYS_WRITE);
        state.write_reg(REG_A0, 1);
        state.write_reg(REG_A1, data_addr);
        state.write_reg(REG_A2, 1);
        dispatch_syscall(&mut state, &runtime, &mut commit).unwrap();
        assert_eq!(state.read_reg(REG_A0), (-(GUEST_EFAULT as i64)) as u64);

        state.write_reg(REG_A7, SYS_MPROTECT);
        state.write_reg(REG_A0, data_addr);
        state.write_reg(REG_A1, PAGE_SIZE);
        state.write_reg(REG_A2, libc::PROT_READ as u64);
        dispatch_syscall(&mut state, &runtime, &mut commit).unwrap();
        assert_eq!(state.read_reg(REG_A0), 0);

        state.write_reg(REG_A7, SYS_WRITE);
        state.write_reg(REG_A0, 1);
        state.write_reg(REG_A1, data_addr);
        state.write_reg(REG_A2, 1);
        dispatch_syscall(&mut state, &runtime, &mut commit).unwrap();
        assert_eq!(state.read_reg(REG_A0), 1);
    }

    #[test]
    fn rt_sigprocmask_zeroes_old_mask() {
        let mask_addr = 0x4000u64;
        let data = vec![runtime::MemoryRegion {
            base: mask_addr,
            size: PAGE_SIZE,
            flags: MEM_READ | MEM_WRITE,
            data: vec![0xFF; PAGE_SIZE as usize],
        }];
        let runtime = sample_runtime(&[enc_acrc(1)], &data);
        let mut state = ExecState::from_runtime(&runtime);
        let mut commit = CommitRecord::unsupported(0, 0, 0, TRAP_ILLEGAL_INST, &runtime.block);

        state.write_reg(REG_A7, SYS_RT_SIGPROCMASK);
        state.write_reg(REG_A0, 0);
        state.write_reg(REG_A1, 0);
        state.write_reg(REG_A2, mask_addr);
        state.write_reg(REG_A3, GUEST_SIGSET_BYTES as u64);
        dispatch_syscall(&mut state, &runtime, &mut commit).unwrap();

        assert_eq!(state.read_reg(REG_A0), 0);
        assert_eq!(
            state
                .memory
                .read_bytes(mask_addr, GUEST_SIGSET_BYTES)
                .unwrap(),
            vec![0u8; GUEST_SIGSET_BYTES]
        );
    }

    #[test]
    fn uname_writes_guest_struct() {
        let uts_addr = 0x4000u64;
        let data = vec![runtime::MemoryRegion {
            base: uts_addr,
            size: PAGE_SIZE,
            flags: MEM_READ | MEM_WRITE,
            data: vec![0u8; PAGE_SIZE as usize],
        }];
        let runtime = sample_runtime(&[enc_acrc(1)], &data);
        let mut state = ExecState::from_runtime(&runtime);
        let mut commit = CommitRecord::unsupported(0, 0, 0, TRAP_ILLEGAL_INST, &runtime.block);

        state.write_reg(REG_A7, SYS_UNAME);
        state.write_reg(REG_A0, uts_addr);
        dispatch_syscall(&mut state, &runtime, &mut commit).unwrap();

        assert_eq!(state.read_reg(REG_A0), 0);
        assert_eq!(state.memory.read_c_string(uts_addr, 64).unwrap(), "Linux");
        assert_eq!(
            state
                .memory
                .read_c_string(uts_addr + (GUEST_UTS_FIELD_BYTES * 4) as u64, 64)
                .unwrap(),
            "linx64"
        );
    }

    #[test]
    fn set_tid_address_and_identity_syscalls_match_runtime_identity() {
        let runtime = sample_runtime(&[enc_acrc(1)], &[]);
        let mut state = ExecState::from_runtime(&runtime);
        let mut commit = CommitRecord::unsupported(0, 0, 0, TRAP_ILLEGAL_INST, &runtime.block);
        let tid_addr = 0x5000u64;

        state.write_reg(REG_A7, SYS_SET_TID_ADDRESS);
        state.write_reg(REG_A0, tid_addr);
        dispatch_syscall(&mut state, &runtime, &mut commit).unwrap();
        let tid = state.read_reg(REG_A0);

        assert!(tid > 0);
        assert_eq!(state.clear_child_tid, tid_addr);

        for number in [SYS_GETUID, SYS_GETEUID, SYS_GETGID, SYS_GETEGID] {
            state.write_reg(REG_A7, number);
            dispatch_syscall(&mut state, &runtime, &mut commit).unwrap();
            assert_eq!(state.read_reg(REG_A0), 0);
        }
    }

    #[test]
    fn getresuid_writes_guest_ids() {
        let ids_addr = 0x4000u64;
        let data = vec![runtime::MemoryRegion {
            base: ids_addr,
            size: PAGE_SIZE,
            flags: MEM_READ | MEM_WRITE,
            data: vec![0u8; PAGE_SIZE as usize],
        }];
        let runtime = sample_runtime(&[enc_acrc(1)], &data);
        let mut state = ExecState::from_runtime(&runtime);
        let mut commit = CommitRecord::unsupported(0, 0, 0, TRAP_ILLEGAL_INST, &runtime.block);

        state.uid = 11;
        state.euid = 22;
        state.suid = 33;
        state.write_reg(REG_A7, SYS_GETRESUID);
        state.write_reg(REG_A0, ids_addr);
        state.write_reg(REG_A1, ids_addr + 4);
        state.write_reg(REG_A2, ids_addr + 8);
        dispatch_syscall(&mut state, &runtime, &mut commit).unwrap();

        assert_eq!(state.read_reg(REG_A0), 0);
        assert_eq!(state.memory.read_u32(ids_addr).unwrap(), 11);
        assert_eq!(state.memory.read_u32(ids_addr + 4).unwrap(), 22);
        assert_eq!(state.memory.read_u32(ids_addr + 8).unwrap(), 33);
    }

    #[test]
    fn setuid_updates_single_process_identity() {
        let runtime = sample_runtime(&[enc_acrc(1)], &[]);
        let mut state = ExecState::from_runtime(&runtime);
        let mut commit = CommitRecord::unsupported(0, 0, 0, TRAP_ILLEGAL_INST, &runtime.block);

        state.uid = 5;
        state.euid = 7;
        state.suid = 5;
        state.write_reg(REG_A7, SYS_SETUID);
        state.write_reg(REG_A0, 5);
        dispatch_syscall(&mut state, &runtime, &mut commit).unwrap();
        assert_eq!(state.read_reg(REG_A0), 0);
        assert_eq!([state.uid, state.euid, state.suid], [5, 5, 5]);

        state.write_reg(REG_A7, SYS_SETUID);
        state.write_reg(REG_A0, 9);
        dispatch_syscall(&mut state, &runtime, &mut commit).unwrap();
        assert_eq!(state.read_reg(REG_A0), (-(GUEST_EPERM as i64)) as u64);
    }

    #[test]
    fn setresgid_updates_selected_fields() {
        let runtime = sample_runtime(&[enc_acrc(1)], &[]);
        let mut state = ExecState::from_runtime(&runtime);
        let mut commit = CommitRecord::unsupported(0, 0, 0, TRAP_ILLEGAL_INST, &runtime.block);

        state.gid = 11;
        state.egid = 22;
        state.sgid = 33;
        state.write_reg(REG_A7, SYS_SETRESGID);
        state.write_reg(REG_A0, u64::MAX);
        state.write_reg(REG_A1, 11);
        state.write_reg(REG_A2, 0);
        dispatch_syscall(&mut state, &runtime, &mut commit).unwrap();

        assert_eq!(state.read_reg(REG_A0), 0);
        assert_eq!([state.gid, state.egid, state.sgid], [11, 11, 0]);
    }

    #[test]
    fn futex_wait_and_wake_follow_single_process_contract() {
        let futex_addr = 0x4000u64;
        let data = vec![runtime::MemoryRegion {
            base: futex_addr,
            size: PAGE_SIZE,
            flags: MEM_READ | MEM_WRITE,
            data: vec![0u8; PAGE_SIZE as usize],
        }];
        let runtime = sample_runtime(&[enc_acrc(1)], &data);
        let mut state = ExecState::from_runtime(&runtime);
        let mut commit = CommitRecord::unsupported(0, 0, 0, TRAP_ILLEGAL_INST, &runtime.block);

        state.memory.write_u32_checked(futex_addr, 7).unwrap();

        state.write_reg(REG_A7, SYS_FUTEX);
        state.write_reg(REG_A0, futex_addr);
        state.write_reg(REG_A1, (FUTEX_WAIT | FUTEX_PRIVATE) as u64);
        state.write_reg(REG_A2, 3);
        state.write_reg(REG_A3, 0);
        dispatch_syscall(&mut state, &runtime, &mut commit).unwrap();
        assert_eq!(state.read_reg(REG_A0), (-(GUEST_EAGAIN as i64)) as u64);

        state.write_reg(REG_A7, SYS_FUTEX);
        state.write_reg(REG_A0, futex_addr);
        state.write_reg(REG_A1, (FUTEX_WAKE | FUTEX_PRIVATE) as u64);
        state.write_reg(REG_A2, 1);
        dispatch_syscall(&mut state, &runtime, &mut commit).unwrap();
        assert_eq!(state.read_reg(REG_A0), 0);
    }

    #[test]
    fn sysinfo_writes_linux_guest_layout() {
        let info_addr = 0x4000u64;
        let data = vec![runtime::MemoryRegion {
            base: info_addr,
            size: PAGE_SIZE,
            flags: MEM_READ | MEM_WRITE,
            data: vec![0u8; PAGE_SIZE as usize],
        }];
        let runtime = sample_runtime(&[enc_acrc(1)], &data);
        let mut state = ExecState::from_runtime(&runtime);
        let mut commit = CommitRecord::unsupported(0, 0, 0, TRAP_ILLEGAL_INST, &runtime.block);

        state.write_reg(REG_A7, SYS_SYSINFO);
        state.write_reg(REG_A0, info_addr);
        dispatch_syscall(&mut state, &runtime, &mut commit).unwrap();

        assert_eq!(state.read_reg(REG_A0), 0);
        assert_eq!(
            state.memory.read_u64(info_addr + 32).unwrap(),
            runtime.config.mem_bytes
        );
        assert_eq!(state.memory.read_u32(info_addr + 104).unwrap(), 1);
        assert_eq!(state.memory.read_u16(info_addr + 80).unwrap(), 1);
    }

    #[test]
    fn prlimit64_reads_and_updates_current_process_limits() {
        let limit_addr = 0x4000u64;
        let data = vec![runtime::MemoryRegion {
            base: limit_addr,
            size: PAGE_SIZE,
            flags: MEM_READ | MEM_WRITE,
            data: vec![0u8; PAGE_SIZE as usize],
        }];
        let runtime = sample_runtime(&[enc_acrc(1)], &data);
        let mut state = ExecState::from_runtime(&runtime);
        let mut commit = CommitRecord::unsupported(0, 0, 0, TRAP_ILLEGAL_INST, &runtime.block);

        state.write_reg(REG_A7, SYS_PRLIMIT64);
        state.write_reg(REG_A0, 0);
        state.write_reg(REG_A1, RLIMIT_STACK as u64);
        state.write_reg(REG_A2, 0);
        state.write_reg(REG_A3, limit_addr);
        dispatch_syscall(&mut state, &runtime, &mut commit).unwrap();
        assert_eq!(state.read_reg(REG_A0), 0);

        let old_cur = state.memory.read_u64(limit_addr).unwrap();
        let old_max = state.memory.read_u64(limit_addr + 8).unwrap();
        assert_eq!(old_cur, runtime.config.stack_size);
        assert_eq!(old_max, runtime.config.stack_size);

        state
            .memory
            .write_u64_checked(limit_addr, old_cur / 2)
            .unwrap();
        state
            .memory
            .write_u64_checked(limit_addr + 8, old_max)
            .unwrap();

        state.write_reg(REG_A7, SYS_PRLIMIT64);
        state.write_reg(REG_A0, 0);
        state.write_reg(REG_A1, RLIMIT_STACK as u64);
        state.write_reg(REG_A2, limit_addr);
        state.write_reg(REG_A3, 0);
        dispatch_syscall(&mut state, &runtime, &mut commit).unwrap();
        assert_eq!(state.read_reg(REG_A0), 0);
        assert_eq!(state.rlimit_for(RLIMIT_STACK).unwrap().cur, old_cur / 2);
    }

    #[test]
    fn getcwd_and_getrandom_write_guest_buffers() {
        let cwd_addr = 0x4000u64;
        let rand_addr = 0x5000u64;
        let data = vec![
            runtime::MemoryRegion {
                base: cwd_addr,
                size: PAGE_SIZE,
                flags: MEM_READ | MEM_WRITE,
                data: vec![0u8; PAGE_SIZE as usize],
            },
            runtime::MemoryRegion {
                base: rand_addr,
                size: PAGE_SIZE,
                flags: MEM_READ | MEM_WRITE,
                data: vec![0u8; PAGE_SIZE as usize],
            },
        ];
        let mut runtime = sample_runtime(&[enc_acrc(1)], &data);
        runtime.config.workdir = Some(PathBuf::from("/tmp/linxcoremodel"));
        let mut state = ExecState::from_runtime(&runtime);
        let mut commit = CommitRecord::unsupported(0, 0, 0, TRAP_ILLEGAL_INST, &runtime.block);

        state.write_reg(REG_A7, SYS_GETCWD);
        state.write_reg(REG_A0, cwd_addr);
        state.write_reg(REG_A1, 64);
        dispatch_syscall(&mut state, &runtime, &mut commit).unwrap();
        assert_eq!(state.read_reg(REG_A0), cwd_addr);
        assert_eq!(
            state.memory.read_c_string(cwd_addr, 63).unwrap(),
            "/tmp/linxcoremodel"
        );

        state.write_reg(REG_A7, SYS_GETRANDOM);
        state.write_reg(REG_A0, rand_addr);
        state.write_reg(REG_A1, 16);
        state.write_reg(REG_A2, 0);
        dispatch_syscall(&mut state, &runtime, &mut commit).unwrap();
        assert_eq!(state.read_reg(REG_A0), 16);
        assert_ne!(
            state.memory.read_bytes(rand_addr, 16).unwrap(),
            vec![0u8; 16]
        );
    }

    #[test]
    fn prctl_round_trips_thread_name() {
        let name_addr = 0x4000u64;
        let out_addr = 0x5000u64;
        let data = vec![
            runtime::MemoryRegion {
                base: name_addr,
                size: PAGE_SIZE,
                flags: MEM_READ | MEM_WRITE,
                data: {
                    let mut bytes = vec![0u8; PAGE_SIZE as usize];
                    bytes[..8].copy_from_slice(b"worker-0");
                    bytes[8] = 0;
                    bytes
                },
            },
            runtime::MemoryRegion {
                base: out_addr,
                size: PAGE_SIZE,
                flags: MEM_READ | MEM_WRITE,
                data: vec![0u8; PAGE_SIZE as usize],
            },
        ];
        let runtime = sample_runtime(&[enc_acrc(1)], &data);
        let mut state = ExecState::from_runtime(&runtime);
        let mut commit = CommitRecord::unsupported(0, 0, 0, TRAP_ILLEGAL_INST, &runtime.block);

        state.write_reg(REG_A7, SYS_PRCTL);
        state.write_reg(REG_A0, GUEST_PR_SET_NAME);
        state.write_reg(REG_A1, name_addr);
        dispatch_syscall(&mut state, &runtime, &mut commit).unwrap();
        assert_eq!(state.read_reg(REG_A0), 0);

        state.write_reg(REG_A7, SYS_PRCTL);
        state.write_reg(REG_A0, GUEST_PR_GET_NAME);
        state.write_reg(REG_A1, out_addr);
        dispatch_syscall(&mut state, &runtime, &mut commit).unwrap();
        assert_eq!(state.read_reg(REG_A0), 0);
        assert_eq!(
            state
                .memory
                .read_c_string(out_addr, GUEST_PRCTL_NAME_BYTES)
                .unwrap(),
            "worker-0"
        );
    }

    #[test]
    fn madvise_and_membarrier_follow_model_contract() {
        let map_addr = 0x6000u64;
        let data = vec![runtime::MemoryRegion {
            base: map_addr,
            size: PAGE_SIZE,
            flags: MEM_READ | MEM_WRITE,
            data: vec![0u8; PAGE_SIZE as usize],
        }];
        let runtime = sample_runtime(&[enc_acrc(1)], &data);
        let mut state = ExecState::from_runtime(&runtime);
        let mut commit = CommitRecord::unsupported(0, 0, 0, TRAP_ILLEGAL_INST, &runtime.block);

        state.write_reg(REG_A7, SYS_MADVISE);
        state.write_reg(REG_A0, map_addr);
        state.write_reg(REG_A1, PAGE_SIZE);
        state.write_reg(REG_A2, 4);
        dispatch_syscall(&mut state, &runtime, &mut commit).unwrap();
        assert_eq!(state.read_reg(REG_A0), 0);

        state.write_reg(REG_A7, SYS_MEMBARRIER);
        state.write_reg(REG_A0, GUEST_MEMBARRIER_CMD_QUERY);
        state.write_reg(REG_A1, 0);
        dispatch_syscall(&mut state, &runtime, &mut commit).unwrap();
        let query = state.read_reg(REG_A0);
        assert_ne!(query & GUEST_MEMBARRIER_CMD_PRIVATE_EXPEDITED, 0);
        assert_ne!(query & GUEST_MEMBARRIER_CMD_REGISTER_PRIVATE_EXPEDITED, 0);

        state.write_reg(REG_A7, SYS_MEMBARRIER);
        state.write_reg(REG_A0, GUEST_MEMBARRIER_CMD_REGISTER_PRIVATE_EXPEDITED);
        state.write_reg(REG_A1, 0);
        dispatch_syscall(&mut state, &runtime, &mut commit).unwrap();
        assert_eq!(state.read_reg(REG_A0), 0);

        state.write_reg(REG_A7, SYS_MEMBARRIER);
        state.write_reg(REG_A0, GUEST_MEMBARRIER_CMD_PRIVATE_EXPEDITED);
        state.write_reg(REG_A1, 0);
        dispatch_syscall(&mut state, &runtime, &mut commit).unwrap();
        assert_eq!(state.read_reg(REG_A0), 0);
    }

    #[test]
    fn rseq_registration_initializes_guest_abi() {
        let rseq_addr = 0x7000u64;
        let data = vec![runtime::MemoryRegion {
            base: rseq_addr,
            size: PAGE_SIZE,
            flags: MEM_READ | MEM_WRITE,
            data: vec![0xAA; PAGE_SIZE as usize],
        }];
        let runtime = sample_runtime(&[enc_acrc(1)], &data);
        let mut state = ExecState::from_runtime(&runtime);
        let mut commit = CommitRecord::unsupported(0, 0, 0, TRAP_ILLEGAL_INST, &runtime.block);

        state.write_reg(REG_A7, SYS_RSEQ);
        state.write_reg(REG_A0, rseq_addr);
        state.write_reg(REG_A1, GUEST_RSEQ_MIN_LEN as u64);
        state.write_reg(REG_A2, 0);
        state.write_reg(REG_A3, GUEST_RSEQ_SIG as u64);
        dispatch_syscall(&mut state, &runtime, &mut commit).unwrap();

        assert_eq!(state.read_reg(REG_A0), 0);
        assert_eq!(state.rseq_addr, rseq_addr);
        assert_eq!(state.rseq_len, GUEST_RSEQ_MIN_LEN);
        assert_eq!(state.rseq_sig, GUEST_RSEQ_SIG);
        assert_eq!(state.memory.read_u32(rseq_addr).unwrap(), 0);
        assert_eq!(state.memory.read_u32(rseq_addr + 4).unwrap(), 0);
        assert_eq!(state.memory.read_u64(rseq_addr + 8).unwrap(), 0);

        state.write_reg(REG_A7, SYS_RSEQ);
        state.write_reg(REG_A0, rseq_addr);
        state.write_reg(REG_A1, GUEST_RSEQ_MIN_LEN as u64);
        state.write_reg(REG_A2, GUEST_RSEQ_FLAG_UNREGISTER);
        state.write_reg(REG_A3, GUEST_RSEQ_SIG as u64);
        dispatch_syscall(&mut state, &runtime, &mut commit).unwrap();
        assert_eq!(state.read_reg(REG_A0), 0);
        assert_eq!(state.rseq_addr, 0);
    }

    #[test]
    fn pipe2_and_dup3_roundtrip_guest_data() {
        let pipefd_addr = 0x4000u64;
        let write_addr = 0x5000u64;
        let read_addr = 0x6000u64;
        let data = vec![
            runtime::MemoryRegion {
                base: pipefd_addr,
                size: PAGE_SIZE,
                flags: MEM_READ | MEM_WRITE,
                data: vec![0u8; PAGE_SIZE as usize],
            },
            runtime::MemoryRegion {
                base: write_addr,
                size: PAGE_SIZE,
                flags: MEM_READ | MEM_WRITE,
                data: {
                    let mut bytes = vec![0u8; PAGE_SIZE as usize];
                    bytes[..3].copy_from_slice(b"abc");
                    bytes
                },
            },
            runtime::MemoryRegion {
                base: read_addr,
                size: PAGE_SIZE,
                flags: MEM_READ | MEM_WRITE,
                data: vec![0u8; PAGE_SIZE as usize],
            },
        ];
        let runtime = sample_runtime(&[enc_acrc(1)], &data);
        let mut state = ExecState::from_runtime(&runtime);
        let mut commit = CommitRecord::unsupported(0, 0, 0, TRAP_ILLEGAL_INST, &runtime.block);

        state.write_reg(REG_A7, SYS_PIPE2);
        state.write_reg(REG_A0, pipefd_addr);
        state.write_reg(REG_A1, GUEST_O_CLOEXEC as u64);
        dispatch_syscall(&mut state, &runtime, &mut commit).unwrap();
        assert_eq!(state.read_reg(REG_A0), 0);

        let read_guest_fd = state.memory.read_u32(pipefd_addr).unwrap() as u64;
        let write_guest_fd = state.memory.read_u32(pipefd_addr + 4).unwrap() as u64;
        assert_eq!(
            state.fd_fd_flags.get(&read_guest_fd).copied().unwrap_or(0),
            GUEST_FD_CLOEXEC
        );
        assert_eq!(
            state.fd_fd_flags.get(&write_guest_fd).copied().unwrap_or(0),
            GUEST_FD_CLOEXEC
        );

        state.write_reg(REG_A7, SYS_DUP3);
        state.write_reg(REG_A0, write_guest_fd);
        state.write_reg(REG_A1, 20);
        state.write_reg(REG_A2, GUEST_O_CLOEXEC as u64);
        dispatch_syscall(&mut state, &runtime, &mut commit).unwrap();
        assert_eq!(state.read_reg(REG_A0), 20);
        assert_eq!(
            state.fd_fd_flags.get(&20).copied().unwrap_or(0),
            GUEST_FD_CLOEXEC
        );

        state.write_reg(REG_A7, SYS_WRITE);
        state.write_reg(REG_A0, 20);
        state.write_reg(REG_A1, write_addr);
        state.write_reg(REG_A2, 3);
        dispatch_syscall(&mut state, &runtime, &mut commit).unwrap();
        assert_eq!(state.read_reg(REG_A0), 3);

        state.write_reg(REG_A7, SYS_READ);
        state.write_reg(REG_A0, read_guest_fd);
        state.write_reg(REG_A1, read_addr);
        state.write_reg(REG_A2, 3);
        dispatch_syscall(&mut state, &runtime, &mut commit).unwrap();
        assert_eq!(state.read_reg(REG_A0), 3);
        assert_eq!(state.memory.read_bytes(read_addr, 3).unwrap(), b"abc");
    }

    #[test]
    fn ppoll_reports_guest_pipe_readiness() {
        let pipefd_addr = 0x4000u64;
        let timeout_addr = 0x5000u64;
        let read_addr = 0x6000u64;
        let data = vec![
            runtime::MemoryRegion {
                base: pipefd_addr,
                size: PAGE_SIZE,
                flags: MEM_READ | MEM_WRITE,
                data: vec![0u8; PAGE_SIZE as usize],
            },
            runtime::MemoryRegion {
                base: timeout_addr,
                size: PAGE_SIZE,
                flags: MEM_READ | MEM_WRITE,
                data: vec![0u8; PAGE_SIZE as usize],
            },
            runtime::MemoryRegion {
                base: read_addr,
                size: PAGE_SIZE,
                flags: MEM_READ | MEM_WRITE,
                data: vec![0u8; PAGE_SIZE as usize],
            },
        ];
        let runtime = sample_runtime(&[enc_acrc(1)], &data);
        let mut state = ExecState::from_runtime(&runtime);
        let mut commit = CommitRecord::unsupported(0, 0, 0, TRAP_ILLEGAL_INST, &runtime.block);

        state.write_reg(REG_A7, SYS_PIPE2);
        state.write_reg(REG_A0, pipefd_addr);
        state.write_reg(REG_A1, 0);
        dispatch_syscall(&mut state, &runtime, &mut commit).unwrap();
        let read_guest_fd = state.memory.read_u32(pipefd_addr).unwrap() as u64;
        let write_guest_fd = state.memory.read_u32(pipefd_addr + 4).unwrap() as u64;

        state
            .memory
            .write_u32_checked(pipefd_addr, read_guest_fd as u32)
            .unwrap();
        state
            .memory
            .write_u16_checked(pipefd_addr + 4, 0x001)
            .unwrap();
        state.memory.write_u16_checked(pipefd_addr + 6, 0).unwrap();

        state.write_reg(REG_A7, SYS_WRITE);
        state.write_reg(REG_A0, write_guest_fd);
        state.write_reg(REG_A1, read_addr);
        state.write_reg(REG_A2, 1);
        state.memory.write_bytes_checked(read_addr, b"x").unwrap();
        dispatch_syscall(&mut state, &runtime, &mut commit).unwrap();

        state.write_reg(REG_A7, SYS_PPOLL);
        state.write_reg(REG_A0, pipefd_addr);
        state.write_reg(REG_A1, 1);
        state.write_reg(REG_A2, timeout_addr);
        state.write_reg(REG_A3, 0);
        state.write_reg(REG_A4, 0);
        dispatch_syscall(&mut state, &runtime, &mut commit).unwrap();

        assert_eq!(state.read_reg(REG_A0), 1);
        assert_ne!(state.memory.read_u16(pipefd_addr + 6).unwrap() & 0x001, 0);
    }

    #[test]
    fn pselect6_reports_guest_read_set_readiness() {
        let readfds_addr = 0x4000u64;
        let timeout_addr = 0x5000u64;
        let sigdata_addr = 0x6000u64;
        let byte_addr = 0x7000u64;
        let data = vec![
            runtime::MemoryRegion {
                base: readfds_addr,
                size: PAGE_SIZE,
                flags: MEM_READ | MEM_WRITE,
                data: vec![0u8; PAGE_SIZE as usize],
            },
            runtime::MemoryRegion {
                base: timeout_addr,
                size: PAGE_SIZE,
                flags: MEM_READ | MEM_WRITE,
                data: vec![0u8; PAGE_SIZE as usize],
            },
            runtime::MemoryRegion {
                base: sigdata_addr,
                size: PAGE_SIZE,
                flags: MEM_READ | MEM_WRITE,
                data: vec![0u8; PAGE_SIZE as usize],
            },
            runtime::MemoryRegion {
                base: byte_addr,
                size: PAGE_SIZE,
                flags: MEM_READ | MEM_WRITE,
                data: vec![0u8; PAGE_SIZE as usize],
            },
        ];
        let runtime = sample_runtime(&[enc_acrc(1)], &data);
        let mut state = ExecState::from_runtime(&runtime);
        let mut commit = CommitRecord::unsupported(0, 0, 0, TRAP_ILLEGAL_INST, &runtime.block);

        state.write_reg(REG_A7, SYS_PIPE2);
        state.write_reg(REG_A0, byte_addr);
        state.write_reg(REG_A1, 0);
        dispatch_syscall(&mut state, &runtime, &mut commit).unwrap();
        let read_guest_fd = state.memory.read_u32(byte_addr).unwrap() as u64;
        let write_guest_fd = state.memory.read_u32(byte_addr + 4).unwrap() as u64;

        let bit = 1u64 << (read_guest_fd % 64);
        state.memory.write_u64_checked(readfds_addr, bit).unwrap();
        state.memory.write_u64_checked(timeout_addr, 0).unwrap();
        state.memory.write_u64_checked(timeout_addr + 8, 0).unwrap();
        state.memory.write_u64_checked(sigdata_addr, 0).unwrap();
        state.memory.write_u64_checked(sigdata_addr + 8, 0).unwrap();
        state
            .memory
            .write_bytes_checked(byte_addr + 16, b"z")
            .unwrap();

        state.write_reg(REG_A7, SYS_WRITE);
        state.write_reg(REG_A0, write_guest_fd);
        state.write_reg(REG_A1, byte_addr + 16);
        state.write_reg(REG_A2, 1);
        dispatch_syscall(&mut state, &runtime, &mut commit).unwrap();

        state.write_reg(REG_A7, SYS_PSELECT6);
        state.write_reg(REG_A0, read_guest_fd + 1);
        state.write_reg(REG_A1, readfds_addr);
        state.write_reg(REG_A2, 0);
        state.write_reg(REG_A3, 0);
        state.write_reg(REG_A4, timeout_addr);
        state.write_reg(REG_A5, sigdata_addr);
        dispatch_syscall(&mut state, &runtime, &mut commit).unwrap();

        assert_eq!(state.read_reg(REG_A0), 1);
        assert_eq!(state.memory.read_u64(readfds_addr).unwrap() & bit, bit);
    }

    #[test]
    fn epoll_pwait_reports_eventfd_readiness() {
        let value_addr = 0x4000u64;
        let ctl_addr = 0x5000u64;
        let out_addr = 0x6000u64;
        let read_addr = 0x7000u64;
        let data = vec![
            runtime::MemoryRegion {
                base: value_addr,
                size: PAGE_SIZE,
                flags: MEM_READ | MEM_WRITE,
                data: vec![0u8; PAGE_SIZE as usize],
            },
            runtime::MemoryRegion {
                base: ctl_addr,
                size: PAGE_SIZE,
                flags: MEM_READ | MEM_WRITE,
                data: vec![0u8; PAGE_SIZE as usize],
            },
            runtime::MemoryRegion {
                base: out_addr,
                size: PAGE_SIZE,
                flags: MEM_READ | MEM_WRITE,
                data: vec![0u8; PAGE_SIZE as usize],
            },
            runtime::MemoryRegion {
                base: read_addr,
                size: PAGE_SIZE,
                flags: MEM_READ | MEM_WRITE,
                data: vec![0u8; PAGE_SIZE as usize],
            },
        ];
        let runtime = sample_runtime(&[enc_acrc(1)], &data);
        let mut state = ExecState::from_runtime(&runtime);
        let mut commit = CommitRecord::unsupported(0, 0, 0, TRAP_ILLEGAL_INST, &runtime.block);

        state.write_reg(REG_A7, SYS_EVENTFD2);
        state.write_reg(REG_A0, 0);
        state.write_reg(REG_A1, 0);
        dispatch_syscall(&mut state, &runtime, &mut commit).unwrap();
        let event_fd = state.read_reg(REG_A0);

        state.write_reg(REG_A7, SYS_EPOLL_CREATE1);
        state.write_reg(REG_A0, 0);
        dispatch_syscall(&mut state, &runtime, &mut commit).unwrap();
        let epoll_fd = state.read_reg(REG_A0);

        write_guest_epoll_event(
            &mut state.memory,
            ctl_addr,
            GUEST_EPOLLIN,
            0x1122_3344_5566_7788,
        )
        .unwrap();
        state.write_reg(REG_A7, SYS_EPOLL_CTL);
        state.write_reg(REG_A0, epoll_fd);
        state.write_reg(REG_A1, GUEST_EPOLL_CTL_ADD as u64);
        state.write_reg(REG_A2, event_fd);
        state.write_reg(REG_A3, ctl_addr);
        dispatch_syscall(&mut state, &runtime, &mut commit).unwrap();
        assert_eq!(state.read_reg(REG_A0), 0);

        state.memory.write_u64_checked(value_addr, 7).unwrap();
        state.write_reg(REG_A7, SYS_WRITE);
        state.write_reg(REG_A0, event_fd);
        state.write_reg(REG_A1, value_addr);
        state.write_reg(REG_A2, 8);
        dispatch_syscall(&mut state, &runtime, &mut commit).unwrap();
        assert_eq!(state.read_reg(REG_A0), 8);

        state.write_reg(REG_A7, SYS_EPOLL_PWAIT);
        state.write_reg(REG_A0, epoll_fd);
        state.write_reg(REG_A1, out_addr);
        state.write_reg(REG_A2, 4);
        state.write_reg(REG_A3, 0);
        state.write_reg(REG_A4, 0);
        state.write_reg(REG_A5, 0);
        dispatch_syscall(&mut state, &runtime, &mut commit).unwrap();
        assert_eq!(state.read_reg(REG_A0), 1);
        assert_ne!(state.memory.read_u32(out_addr).unwrap() & GUEST_EPOLLIN, 0);
        assert_eq!(
            state.memory.read_u64(out_addr + 8).unwrap(),
            0x1122_3344_5566_7788
        );

        state.write_reg(REG_A7, SYS_READ);
        state.write_reg(REG_A0, event_fd);
        state.write_reg(REG_A1, read_addr);
        state.write_reg(REG_A2, 8);
        dispatch_syscall(&mut state, &runtime, &mut commit).unwrap();
        assert_eq!(state.read_reg(REG_A0), 8);
        assert_eq!(state.memory.read_u64(read_addr).unwrap(), 7);
    }

    #[test]
    fn wait4_reports_no_child_in_single_process_mode() {
        let status_addr = 0x4000u64;
        let rusage_addr = 0x5000u64;
        let data = vec![
            runtime::MemoryRegion {
                base: status_addr,
                size: PAGE_SIZE,
                flags: MEM_READ | MEM_WRITE,
                data: vec![0xAA; PAGE_SIZE as usize],
            },
            runtime::MemoryRegion {
                base: rusage_addr,
                size: PAGE_SIZE,
                flags: MEM_READ | MEM_WRITE,
                data: vec![0xBB; PAGE_SIZE as usize],
            },
        ];
        let runtime = sample_runtime(&[enc_acrc(1)], &data);
        let mut state = ExecState::from_runtime(&runtime);
        let mut commit = CommitRecord::unsupported(0, 0, 0, TRAP_ILLEGAL_INST, &runtime.block);

        state.write_reg(REG_A7, SYS_WAIT4);
        state.write_reg(REG_A0, u64::MAX);
        state.write_reg(REG_A1, status_addr);
        state.write_reg(REG_A2, 0);
        state.write_reg(REG_A3, rusage_addr);
        dispatch_syscall(&mut state, &runtime, &mut commit).unwrap();

        assert_eq!(state.read_reg(REG_A0), (-(GUEST_ECHILD as i64)) as u64);
        assert_eq!(state.memory.read_u32(status_addr).unwrap(), 0xAAAA_AAAA);
        assert_eq!(state.memory.read_u32(rusage_addr).unwrap(), 0xBBBB_BBBB);
    }

    #[test]
    fn hl_bstart_std_uses_byte_scaled_target_offsets() {
        let runtime = sample_runtime(&[enc_acrc(1)], &[]);
        let mut state = ExecState::from_runtime(&runtime);
        let pc = 0x11158u64;
        let fallthrough = pc + 6;
        let bundle = u64::from_le_bytes([0xfe, 0xff, 0x01, 0x40, 0xf6, 0xff, 0x56, 0x55]);
        let decoded = decode_word(bundle).unwrap();
        let mut commit = empty_commit(0, pc, &decoded, &runtime);
        state.pc = pc;

        let outcome = execute_step(&mut state, &runtime, &decoded, &mut commit).unwrap();

        assert_eq!(outcome.next_pc, fallthrough);
        assert_eq!(
            state.block.as_ref().map(|block| block.kind),
            Some(BlockKind::Call)
        );
        assert_eq!(
            state.block.as_ref().and_then(|block| block.target),
            Some(0x11130)
        );
    }

    #[test]
    fn fret_stk_clears_stale_block_state() {
        let stack_addr = 0x4000u64;
        let data = vec![runtime::MemoryRegion {
            base: stack_addr,
            size: PAGE_SIZE,
            flags: MEM_READ | MEM_WRITE,
            data: vec![0u8; PAGE_SIZE as usize],
        }];
        let runtime = sample_runtime(&[enc_acrc(1)], &data);
        let mut state = ExecState::from_runtime(&runtime);
        let mut commit = CommitRecord::unsupported(0, 0, 0, TRAP_ILLEGAL_INST, &runtime.block);

        state.write_reg(REG_SP, stack_addr);
        state.memory.write_u64_checked(stack_addr, 0x1234).unwrap();
        state.block = Some(BlockContext {
            kind: BlockKind::Cond,
            target: Some(0x3000),
            return_target: Some(0x4000),
        });
        state.cond = true;
        state.carg = true;
        state.target = 0x5000;

        let target = apply_fret_stk(&mut state, &mut commit, REG_RA, REG_RA, 8).unwrap();

        assert_eq!(target, 0x1234);
        assert!(state.block.is_none());
        assert!(!state.cond);
        assert!(!state.carg);
        assert_eq!(state.target, 0);
    }

    #[test]
    fn sigaltstack_round_trips_single_thread_state() {
        let new_addr = 0x4000u64;
        let old_addr = 0x5000u64;
        let data = vec![
            runtime::MemoryRegion {
                base: new_addr,
                size: PAGE_SIZE,
                flags: MEM_READ | MEM_WRITE,
                data: vec![0u8; PAGE_SIZE as usize],
            },
            runtime::MemoryRegion {
                base: old_addr,
                size: PAGE_SIZE,
                flags: MEM_READ | MEM_WRITE,
                data: vec![0u8; PAGE_SIZE as usize],
            },
        ];
        let runtime = sample_runtime(&[enc_acrc(1)], &data);
        let mut state = ExecState::from_runtime(&runtime);
        let mut commit = CommitRecord::unsupported(0, 0, 0, TRAP_ILLEGAL_INST, &runtime.block);

        write_guest_sigaltstack(&mut state.memory, new_addr, 0x8000, 0, 4096).unwrap();
        state.write_reg(REG_A7, SYS_SIGALTSTACK);
        state.write_reg(REG_A0, new_addr);
        state.write_reg(REG_A1, old_addr);
        dispatch_syscall(&mut state, &runtime, &mut commit).unwrap();

        assert_eq!(state.read_reg(REG_A0), 0);
        assert_eq!(state.alt_stack_sp, 0x8000);
        assert_eq!(state.alt_stack_size, 4096);
        assert_eq!(state.alt_stack_flags, 0);
        assert_eq!(
            state.memory.read_u32(old_addr + 8).unwrap(),
            GUEST_SS_DISABLE
        );

        state.write_reg(REG_A7, SYS_SIGALTSTACK);
        state.write_reg(REG_A0, 0);
        state.write_reg(REG_A1, old_addr);
        dispatch_syscall(&mut state, &runtime, &mut commit).unwrap();
        assert_eq!(state.memory.read_u64(old_addr).unwrap(), 0x8000);
        assert_eq!(state.memory.read_u64(old_addr + 16).unwrap(), 4096);
    }

    #[test]
    fn ioctl_tty_requests_use_deterministic_stdio_model() {
        let winsize_addr = 0x4000u64;
        let pgrp_addr = 0x5000u64;
        let data = vec![
            runtime::MemoryRegion {
                base: winsize_addr,
                size: PAGE_SIZE,
                flags: MEM_READ | MEM_WRITE,
                data: vec![0u8; PAGE_SIZE as usize],
            },
            runtime::MemoryRegion {
                base: pgrp_addr,
                size: PAGE_SIZE,
                flags: MEM_READ | MEM_WRITE,
                data: vec![0u8; PAGE_SIZE as usize],
            },
        ];
        let runtime = sample_runtime(&[enc_acrc(1)], &data);
        let mut state = ExecState::from_runtime(&runtime);
        let mut commit = CommitRecord::unsupported(0, 0, 0, TRAP_ILLEGAL_INST, &runtime.block);

        state.write_reg(REG_A7, SYS_IOCTL);
        state.write_reg(REG_A0, 1);
        state.write_reg(REG_A1, GUEST_TIOCGWINSZ);
        state.write_reg(REG_A2, winsize_addr);
        dispatch_syscall(&mut state, &runtime, &mut commit).unwrap();
        assert_eq!(state.read_reg(REG_A0), 0);
        assert_eq!(state.memory.read_u16(winsize_addr).unwrap(), 24);
        assert_eq!(state.memory.read_u16(winsize_addr + 2).unwrap(), 80);

        state.write_reg(REG_A7, SYS_IOCTL);
        state.write_reg(REG_A0, 1);
        state.write_reg(REG_A1, GUEST_TIOCGPGRP);
        state.write_reg(REG_A2, pgrp_addr);
        dispatch_syscall(&mut state, &runtime, &mut commit).unwrap();
        assert_eq!(
            state.memory.read_u32(pgrp_addr).unwrap(),
            state.current_pid as u32
        );

        state.memory.write_u32_checked(pgrp_addr, 77).unwrap();
        state.write_reg(REG_A7, SYS_IOCTL);
        state.write_reg(REG_A0, 1);
        state.write_reg(REG_A1, GUEST_TIOCSPGRP);
        state.write_reg(REG_A2, pgrp_addr);
        dispatch_syscall(&mut state, &runtime, &mut commit).unwrap();
        assert_eq!(state.read_reg(REG_A0), 0);

        state.write_reg(REG_A7, SYS_IOCTL);
        state.write_reg(REG_A0, 1);
        state.write_reg(REG_A1, GUEST_TIOCGPGRP);
        state.write_reg(REG_A2, pgrp_addr);
        dispatch_syscall(&mut state, &runtime, &mut commit).unwrap();
        assert_eq!(state.memory.read_u32(pgrp_addr).unwrap(), 77);
    }

    #[test]
    fn fcntl_tracks_guest_fd_flags() {
        let runtime = sample_runtime(&[enc_acrc(1)], &[]);
        let mut state = ExecState::from_runtime(&runtime);
        let mut host_file = NamedTempFile::new().unwrap();
        host_file.write_all(b"fcntl\n").unwrap();
        host_file.flush().unwrap();
        let host_fd = unsafe { libc::dup(host_file.as_raw_fd()) };
        assert!(host_fd >= 0);
        let guest_fd = 9u64;
        state.fd_table.insert(guest_fd, host_fd);
        state.fd_status_flags.insert(guest_fd, libc::O_RDONLY);
        state.fd_fd_flags.insert(guest_fd, 0);
        let mut commit = CommitRecord::unsupported(0, 0, 0, TRAP_ILLEGAL_INST, &runtime.block);

        state.write_reg(REG_A7, SYS_FCNTL);
        state.write_reg(REG_A0, guest_fd);
        state.write_reg(REG_A1, GUEST_F_SETFD as u64);
        state.write_reg(REG_A2, GUEST_FD_CLOEXEC as u64);
        dispatch_syscall(&mut state, &runtime, &mut commit).unwrap();
        assert_eq!(state.read_reg(REG_A0), 0);

        state.write_reg(REG_A7, SYS_FCNTL);
        state.write_reg(REG_A0, guest_fd);
        state.write_reg(REG_A1, GUEST_F_GETFD as u64);
        dispatch_syscall(&mut state, &runtime, &mut commit).unwrap();
        assert_eq!(state.read_reg(REG_A0), GUEST_FD_CLOEXEC as u64);

        unsafe {
            libc::close(host_fd);
        }
    }

    #[test]
    fn fcntl_dupfd_commands_allocate_new_guest_fds() {
        let runtime = sample_runtime(&[enc_acrc(1)], &[]);
        let mut state = ExecState::from_runtime(&runtime);
        let mut host_file = NamedTempFile::new().unwrap();
        host_file.write_all(b"dupfd\n").unwrap();
        host_file.flush().unwrap();
        let host_fd = unsafe { libc::dup(host_file.as_raw_fd()) };
        assert!(host_fd >= 0);
        let guest_fd = 9u64;
        state.insert_guest_fd(guest_fd, host_fd, GUEST_O_RDONLY, 0);
        let mut commit = CommitRecord::unsupported(0, 0, 0, TRAP_ILLEGAL_INST, &runtime.block);

        state.write_reg(REG_A7, SYS_FCNTL);
        state.write_reg(REG_A0, guest_fd);
        state.write_reg(REG_A1, GUEST_F_DUPFD as u64);
        state.write_reg(REG_A2, 20);
        dispatch_syscall(&mut state, &runtime, &mut commit).unwrap();
        let dup_fd = state.read_reg(REG_A0);
        assert!(dup_fd >= 20);
        assert_eq!(state.fd_fd_flags.get(&dup_fd).copied().unwrap_or(0), 0);

        state.write_reg(REG_A7, SYS_FCNTL);
        state.write_reg(REG_A0, guest_fd);
        state.write_reg(REG_A1, GUEST_F_DUPFD_CLOEXEC as u64);
        state.write_reg(REG_A2, 30);
        dispatch_syscall(&mut state, &runtime, &mut commit).unwrap();
        let cloexec_fd = state.read_reg(REG_A0);
        assert!(cloexec_fd >= 30);
        assert_eq!(
            state.fd_fd_flags.get(&cloexec_fd).copied().unwrap_or(0),
            GUEST_FD_CLOEXEC
        );
    }

    #[test]
    fn newfstatat_and_readlinkat_follow_host_paths() {
        let temp = tempdir().unwrap();
        let target = temp.path().join("target.txt");
        fs::write(&target, b"symlink fixture bytes").unwrap();
        let link = temp.path().join("link.txt");
        symlink(&target, &link).unwrap();
        let path_bytes = b"link.txt\0";
        let path_addr = 0x4000u64;
        let stat_addr = 0x5000u64;
        let link_addr = 0x6000u64;
        let data = vec![
            runtime::MemoryRegion {
                base: path_addr,
                size: PAGE_SIZE,
                flags: MEM_READ | MEM_WRITE,
                data: {
                    let mut bytes = vec![0u8; PAGE_SIZE as usize];
                    bytes[..path_bytes.len()].copy_from_slice(path_bytes);
                    bytes
                },
            },
            runtime::MemoryRegion {
                base: stat_addr,
                size: PAGE_SIZE,
                flags: MEM_READ | MEM_WRITE,
                data: vec![0u8; PAGE_SIZE as usize],
            },
            runtime::MemoryRegion {
                base: link_addr,
                size: PAGE_SIZE,
                flags: MEM_READ | MEM_WRITE,
                data: vec![0u8; PAGE_SIZE as usize],
            },
        ];
        let mut runtime = sample_runtime(&[enc_acrc(1)], &data);
        runtime.config.workdir = Some(temp.path().to_path_buf());
        let mut state = ExecState::from_runtime(&runtime);
        let mut commit = CommitRecord::unsupported(0, 0, 0, TRAP_ILLEGAL_INST, &runtime.block);

        state.write_reg(REG_A7, SYS_NEWFSTATAT);
        state.write_reg(REG_A0, GUEST_AT_FDCWD as u64);
        state.write_reg(REG_A1, path_addr);
        state.write_reg(REG_A2, stat_addr);
        state.write_reg(REG_A3, GUEST_AT_SYMLINK_NOFOLLOW as u64);
        dispatch_syscall(&mut state, &runtime, &mut commit).unwrap();
        assert_eq!(state.read_reg(REG_A0), 0);
        assert_eq!(
            state.memory.read_u32(stat_addr + 16).unwrap() & libc::S_IFMT as u32,
            libc::S_IFLNK as u32
        );

        state.write_reg(REG_A7, SYS_READLINKAT);
        state.write_reg(REG_A0, GUEST_AT_FDCWD as u64);
        state.write_reg(REG_A1, path_addr);
        state.write_reg(REG_A2, link_addr);
        state.write_reg(REG_A3, 128);
        dispatch_syscall(&mut state, &runtime, &mut commit).unwrap();
        let count = state.read_reg(REG_A0) as usize;
        let resolved =
            String::from_utf8(state.memory.read_bytes(link_addr, count).unwrap()).unwrap();
        assert!(resolved.ends_with("target.txt"));
    }

    fn sample_runtime(words: &[u32], extra_regions: &[runtime::MemoryRegion]) -> GuestRuntime {
        let text_base = 0x1000u64;
        let mut text = Vec::with_capacity(words.len() * 4);
        for word in words {
            text.extend_from_slice(&word.to_le_bytes());
        }

        let mut regions = vec![runtime::MemoryRegion {
            base: text_base,
            size: 0x1000,
            flags: 0b101,
            data: {
                let mut bytes = vec![0; 0x1000];
                bytes[..text.len()].copy_from_slice(&text);
                bytes
            },
        }];
        regions.extend_from_slice(extra_regions);
        regions.push(runtime::MemoryRegion {
            base: 0x0000_7FFF_E000,
            size: 0x2000,
            flags: 0b110,
            data: vec![0; 0x2000],
        });

        GuestRuntime {
            image: LoadedElf {
                path: PathBuf::from("sample.elf"),
                entry: text_base,
                little_endian: true,
                bits: 64,
                machine: 0,
                segments: vec![SegmentImage {
                    vaddr: text_base,
                    mem_size: text.len() as u64,
                    file_size: text.len() as u64,
                    flags: 0b101,
                    data: text,
                }],
            },
            config: RuntimeConfig::default(),
            state: isa::ArchitecturalState::new(text_base),
            block: isa::BlockMeta::default(),
            memory: GuestMemory { regions },
            boot: BootInfo {
                entry_pc: text_base,
                stack_top: 0x0000_7FFF_F000,
                stack_pointer: 0x0000_7FFF_F000,
                argc: 0,
            },
            fd_table: HashMap::from([(0, 0), (1, 1), (2, 2)]),
        }
    }

    fn enc_addi(rd: u32, rs1: u32, imm: u32) -> u32 {
        ((imm & 0x0fff) << 20) | (rs1 << 15) | (rd << 7) | 0x15
    }

    fn enc_lwi(rd: u32, rs1: u32, simm12: i32) -> u32 {
        (((simm12 as u32) & 0x0fff) << 20) | (rs1 << 15) | (2 << 12) | (rd << 7) | 0x19
    }

    fn enc_swi(src: u32, base: u32, simm12: i32) -> u32 {
        let imm = simm12 as u32 & 0x0fff;
        ((imm & 0x7f) << 25)
            | (base << 20)
            | (src << 15)
            | (2 << 12)
            | (((imm >> 7) & 0x1f) << 7)
            | 0x59
    }

    fn enc_acrc(rst: u32) -> u32 {
        ((rst & 0xf) << 20) | 0x302b
    }
}
