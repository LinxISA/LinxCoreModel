use anyhow::{Context, Result, bail};
use elf::LoadedElf;
use isa::{ArchitecturalState, BlockMeta, DEFAULT_MEM_BYTES, DEFAULT_STACK_SIZE};
use libc::{clock_gettime, getpid, pid_t, timespec};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::ffi::CString;
use std::fs;
use std::path::PathBuf;

const SYS_GETCWD: u64 = 17;
const SYS_EVENTFD2: u64 = 19;
const SYS_EPOLL_CREATE1: u64 = 20;
const SYS_EPOLL_CTL: u64 = 21;
const SYS_EPOLL_PWAIT: u64 = 22;
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
const SYS_MMAP: u64 = 222;
const SYS_MUNMAP: u64 = 215;
const SYS_MPROTECT: u64 = 226;
const SYS_WAIT4: u64 = 260;
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
const PAGE_SIZE: u64 = 4096;
pub const MEM_EXEC: u32 = 0b001;
pub const MEM_WRITE: u32 = 0b010;
pub const MEM_READ: u32 = 0b100;
const AT_NULL: u64 = 0;
const AT_PAGESZ: u64 = 6;
const AT_ENTRY: u64 = 9;
const AT_PLATFORM: u64 = 15;
const AT_HWCAP: u64 = 16;
const AT_CLKTCK: u64 = 17;
const AT_UID: u64 = 11;
const AT_EUID: u64 = 12;
const AT_GID: u64 = 13;
const AT_EGID: u64 = 14;
const AT_RANDOM: u64 = 25;
const AT_HWCAP2: u64 = 26;
const AT_HWCAP3: u64 = 29;
const AT_HWCAP4: u64 = 30;
const AT_EXECFN: u64 = 31;
const AT_SYSINFO_EHDR: u64 = 33;
const AT_MINSIGSTKSZ: u64 = 51;
const GUEST_MINSIGSTKSZ: u64 = 2048;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeConfig {
    pub mem_bytes: u64,
    pub stack_size: u64,
    pub args: Vec<String>,
    pub env: BTreeMap<String, String>,
    pub workdir: Option<PathBuf>,
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        Self {
            mem_bytes: DEFAULT_MEM_BYTES,
            stack_size: DEFAULT_STACK_SIZE,
            args: Vec::new(),
            env: BTreeMap::new(),
            workdir: None,
        }
    }
}

impl RuntimeConfig {
    pub fn load(path: impl AsRef<std::path::Path>) -> Result<Self> {
        let path = path.as_ref();
        let text = fs::read_to_string(path)
            .with_context(|| format!("failed to read runtime config {}", path.display()))?;
        toml::from_str(&text)
            .with_context(|| format!("failed to parse runtime config {}", path.display()))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryRegion {
    pub base: u64,
    pub size: u64,
    pub flags: u32,
    pub data: Vec<u8>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GuestMemory {
    pub regions: Vec<MemoryRegion>,
}

impl GuestMemory {
    fn region_containing(&self, addr: u64, count: usize) -> Option<&MemoryRegion> {
        let end = addr.checked_add(count as u64)?;
        self.regions
            .iter()
            .find(|region| addr >= region.base && end <= region.base + region.size)
    }

    fn region_containing_with_flags(
        &self,
        addr: u64,
        count: usize,
        required_flags: u32,
    ) -> Option<&MemoryRegion> {
        let region = self.region_containing(addr, count)?;
        (region.flags & required_flags == required_flags).then_some(region)
    }

    fn region_containing_mut(&mut self, addr: u64, count: usize) -> Option<&mut MemoryRegion> {
        let end = addr.checked_add(count as u64)?;
        self.regions
            .iter_mut()
            .find(|region| addr >= region.base && end <= region.base + region.size)
    }

    fn region_containing_mut_with_flags(
        &mut self,
        addr: u64,
        count: usize,
        required_flags: u32,
    ) -> Option<&mut MemoryRegion> {
        let region = self.region_containing_mut(addr, count)?;
        (region.flags & required_flags == required_flags).then_some(region)
    }

    pub fn read_bytes(&self, pc: u64, count: usize) -> Option<Vec<u8>> {
        let region = self.region_containing(pc, count)?;
        let offset = (pc - region.base) as usize;
        Some(region.data.get(offset..offset + count)?.to_vec())
    }

    pub fn read_bytes_checked(&self, pc: u64, count: usize) -> Option<Vec<u8>> {
        let region = self.region_containing_with_flags(pc, count, MEM_READ)?;
        let offset = (pc - region.base) as usize;
        Some(region.data.get(offset..offset + count)?.to_vec())
    }

    pub fn write_bytes(&mut self, addr: u64, bytes: &[u8]) -> Option<()> {
        let region = self.region_containing_mut(addr, bytes.len())?;
        let offset = (addr - region.base) as usize;
        let dst = region.data.get_mut(offset..offset + bytes.len())?;
        dst.copy_from_slice(bytes);
        Some(())
    }

    pub fn write_bytes_checked(&mut self, addr: u64, bytes: &[u8]) -> Option<()> {
        let region = self.region_containing_mut_with_flags(addr, bytes.len(), MEM_WRITE)?;
        let offset = (addr - region.base) as usize;
        let dst = region.data.get_mut(offset..offset + bytes.len())?;
        dst.copy_from_slice(bytes);
        Some(())
    }

    pub fn read_u8(&self, addr: u64) -> Option<u8> {
        self.read_bytes(addr, 1)
            .and_then(|bytes| bytes.first().copied())
    }

    pub fn read_u8_checked(&self, addr: u64) -> Option<u8> {
        self.read_bytes_checked(addr, 1)
            .and_then(|bytes| bytes.first().copied())
    }

    pub fn read_u16(&self, addr: u64) -> Option<u16> {
        let bytes = self.read_bytes(addr, 2)?;
        Some(u16::from_le_bytes(bytes.try_into().ok()?))
    }

    pub fn read_u16_checked(&self, addr: u64) -> Option<u16> {
        let bytes = self.read_bytes_checked(addr, 2)?;
        Some(u16::from_le_bytes(bytes.try_into().ok()?))
    }

    pub fn read_u32(&self, pc: u64) -> Option<u32> {
        let bytes = self.read_bytes(pc, 4)?;
        Some(u32::from_le_bytes(bytes.try_into().ok()?))
    }

    pub fn read_u32_checked(&self, addr: u64) -> Option<u32> {
        let bytes = self.read_bytes_checked(addr, 4)?;
        Some(u32::from_le_bytes(bytes.try_into().ok()?))
    }

    pub fn read_u64(&self, addr: u64) -> Option<u64> {
        let bytes = self.read_bytes(addr, 8)?;
        Some(u64::from_le_bytes(bytes.try_into().ok()?))
    }

    pub fn read_u64_checked(&self, addr: u64) -> Option<u64> {
        let bytes = self.read_bytes_checked(addr, 8)?;
        Some(u64::from_le_bytes(bytes.try_into().ok()?))
    }

    pub fn read_u64_bundle(&self, pc: u64) -> Option<u64> {
        self.regions.iter().find_map(|region| {
            if pc < region.base || pc >= region.base + region.size {
                return None;
            }
            let offset = (pc - region.base) as usize;
            let available = region.data.len().saturating_sub(offset).min(8);
            if available == 0 {
                return None;
            }
            let mut bytes = [0u8; 8];
            bytes[..available].copy_from_slice(region.data.get(offset..offset + available)?);
            Some(u64::from_le_bytes(bytes))
        })
    }

    pub fn fetch_u64_bundle(&self, pc: u64) -> Option<u64> {
        self.regions.iter().find_map(|region| {
            if region.flags & MEM_EXEC == 0 || pc < region.base || pc >= region.base + region.size {
                return None;
            }
            let offset = (pc - region.base) as usize;
            let available = region.data.len().saturating_sub(offset).min(8);
            if available == 0 {
                return None;
            }
            let mut bytes = [0u8; 8];
            bytes[..available].copy_from_slice(region.data.get(offset..offset + available)?);
            Some(u64::from_le_bytes(bytes))
        })
    }

    pub fn write_u16(&mut self, addr: u64, value: u16) -> Option<()> {
        self.write_bytes(addr, &value.to_le_bytes())
    }

    pub fn write_u16_checked(&mut self, addr: u64, value: u16) -> Option<()> {
        self.write_bytes_checked(addr, &value.to_le_bytes())
    }

    pub fn write_u32(&mut self, addr: u64, value: u32) -> Option<()> {
        self.write_bytes(addr, &value.to_le_bytes())
    }

    pub fn write_u32_checked(&mut self, addr: u64, value: u32) -> Option<()> {
        self.write_bytes_checked(addr, &value.to_le_bytes())
    }

    pub fn write_u64(&mut self, addr: u64, value: u64) -> Option<()> {
        self.write_bytes(addr, &value.to_le_bytes())
    }

    pub fn write_u64_checked(&mut self, addr: u64, value: u64) -> Option<()> {
        self.write_bytes_checked(addr, &value.to_le_bytes())
    }

    pub fn read_c_string(&self, addr: u64, max_len: usize) -> Option<String> {
        let mut bytes = Vec::new();
        for idx in 0..max_len {
            let byte = self.read_u8(addr.checked_add(idx as u64)?)?;
            if byte == 0 {
                return String::from_utf8(bytes).ok();
            }
            bytes.push(byte);
        }
        None
    }

    pub fn read_c_string_checked(&self, addr: u64, max_len: usize) -> Option<String> {
        let mut bytes = Vec::new();
        for idx in 0..max_len {
            let byte = self.read_u8_checked(addr.checked_add(idx as u64)?)?;
            if byte == 0 {
                return String::from_utf8(bytes).ok();
            }
            bytes.push(byte);
        }
        None
    }

    pub fn highest_mapped_address(&self) -> u64 {
        self.regions
            .iter()
            .map(|region| region.base + region.size)
            .max()
            .unwrap_or(0)
    }

    pub fn is_range_mapped(&self, addr: u64, size: u64) -> bool {
        let Some(end) = addr.checked_add(size) else {
            return false;
        };
        if size == 0 {
            return false;
        }

        let mut cursor = addr;
        let mut regions = self.regions.iter().collect::<Vec<_>>();
        regions.sort_by_key(|region| region.base);
        for region in regions {
            if region.base > cursor {
                break;
            }
            let region_end = region.base + region.size;
            if region_end <= cursor {
                continue;
            }
            cursor = region_end.min(end);
            if cursor >= end {
                return true;
            }
        }
        false
    }

    pub fn protect_range(&mut self, addr: u64, size: u64, flags: u32) -> bool {
        if !self.is_range_mapped(addr, size) {
            return false;
        }
        self.remap_range(addr, size, Some(flags));
        true
    }

    pub fn unmap_range(&mut self, addr: u64, size: u64) {
        self.remap_range(addr, size, None);
    }

    fn remap_range(&mut self, addr: u64, size: u64, replacement_flags: Option<u32>) {
        let end = addr.saturating_add(size);
        let mut next_regions = Vec::with_capacity(self.regions.len() + 2);
        for region in &self.regions {
            let region_end = region.base + region.size;
            let overlap_start = region.base.max(addr);
            let overlap_end = region_end.min(end);
            if overlap_start >= overlap_end {
                next_regions.push(region.clone());
                continue;
            }

            if overlap_start > region.base {
                next_regions.push(region_slice(
                    region,
                    region.base,
                    overlap_start,
                    region.flags,
                ));
            }
            if let Some(flags) = replacement_flags {
                next_regions.push(region_slice(region, overlap_start, overlap_end, flags));
            }
            if overlap_end < region_end {
                next_regions.push(region_slice(region, overlap_end, region_end, region.flags));
            }
        }
        next_regions.sort_by_key(|region| region.base);
        self.regions = next_regions;
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BootInfo {
    pub entry_pc: u64,
    pub stack_top: u64,
    pub stack_pointer: u64,
    pub argc: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GuestRuntime {
    pub image: LoadedElf,
    pub config: RuntimeConfig,
    pub state: ArchitecturalState,
    pub block: BlockMeta,
    pub memory: GuestMemory,
    pub boot: BootInfo,
    pub fd_table: HashMap<u64, i32>,
}

impl GuestRuntime {
    pub fn bootstrap(image: LoadedElf, config: RuntimeConfig) -> Result<Self> {
        if config.mem_bytes < config.stack_size {
            bail!(
                "mem_bytes ({}) must be >= stack_size ({})",
                config.mem_bytes,
                config.stack_size
            );
        }

        let mut regions = Vec::new();
        for segment in &image.segments {
            let mut data = segment.data.clone();
            data.resize(segment.mem_size as usize, 0);
            regions.push(MemoryRegion {
                base: segment.vaddr,
                size: segment.mem_size,
                flags: segment.flags,
                data,
            });
        }

        let stack_top = 0x0000_7FFF_F000u64;
        let stack_base = stack_top
            .checked_sub(config.stack_size)
            .context("stack underflow while computing guest stack")?;
        regions.push(MemoryRegion {
            base: stack_base,
            size: config.stack_size,
            flags: 0b110,
            data: vec![0; config.stack_size as usize],
        });

        let mut memory = GuestMemory { regions };
        let argv = build_boot_args(&image, &config);
        let envp = build_boot_env(&config);
        let stack_pointer = initialize_user_stack(
            &mut memory,
            stack_base,
            stack_top,
            &argv,
            &envp,
            image.entry,
        )?;

        let mut state = ArchitecturalState::new(image.entry);
        state.regs[1] = stack_pointer;
        let entry_pc = state.pc;
        let argc = argv.len() as u64;

        Ok(Self {
            image,
            config: config.clone(),
            state,
            block: BlockMeta::default(),
            memory,
            boot: BootInfo {
                entry_pc,
                stack_top,
                stack_pointer,
                argc,
            },
            fd_table: HashMap::from([(0, 0), (1, 1), (2, 2)]),
        })
    }

    pub fn fetch_first_word(&self) -> Result<u32> {
        self.memory
            .read_u32(self.state.pc)
            .with_context(|| format!("no mapped instruction at pc=0x{:016x}", self.state.pc))
    }

    pub fn fetch_bundle(&self, pc: u64) -> Result<u64> {
        self.memory
            .fetch_u64_bundle(pc)
            .with_context(|| format!("no mapped instruction bundle at pc=0x{:016x}", pc))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SyscallEffect {
    Return(u64),
    Exit(i32),
}

#[derive(Debug, Clone, Default)]
pub struct HostSyscallShim;

impl HostSyscallShim {
    pub fn dispatch(&self, number: u64, args: [u64; 6]) -> Result<SyscallEffect> {
        match number {
            SYS_GETCWD => bail!(
                "syscall {} is on the allowlist but still needs guest path marshalling in the executor",
                number
            ),
            SYS_PRCTL | SYS_MEMBARRIER | SYS_RSEQ => bail!(
                "syscall {} is on the allowlist but still needs guest thread/runtime handling in the executor",
                number
            ),
            SYS_GETPID => Ok(SyscallEffect::Return(unsafe { unsafe_pid(getpid()) })),
            SYS_GETPPID => Ok(SyscallEffect::Return(unsafe {
                unsafe_pid(libc::getppid())
            })),
            SYS_GETUID | SYS_GETEUID | SYS_GETGID | SYS_GETEGID => Ok(SyscallEffect::Return(0)),
            SYS_SETUID | SYS_SETGID | SYS_SETRESUID | SYS_GETRESUID | SYS_SETRESGID
            | SYS_GETRESGID => bail!(
                "syscall {} is on the allowlist but still needs guest identity handling in the executor",
                number
            ),
            SYS_GETTID => Ok(SyscallEffect::Return(unsafe { unsafe_pid(getpid()) })),
            SYS_SET_TID_ADDRESS => Ok(SyscallEffect::Return(unsafe { unsafe_pid(getpid()) })),
            SYS_SET_ROBUST_LIST | SYS_SIGALTSTACK | SYS_UNAME | SYS_FUTEX | SYS_PRLIMIT64
            | SYS_SYSINFO | SYS_WAIT4 => bail!(
                "syscall {} is on the allowlist but still needs guest memory marshalling in the executor",
                number
            ),
            SYS_CLOCK_GETTIME => {
                let clk_id: libc::clockid_t = args[0].try_into().unwrap_or(libc::CLOCK_REALTIME);
                let ts = host_clock_gettime(clk_id)?;
                let nsec = (ts.tv_sec as u64).saturating_mul(1_000_000_000) + ts.tv_nsec as u64;
                Ok(SyscallEffect::Return(nsec))
            }
            SYS_EXIT | SYS_EXIT_GROUP => Ok(SyscallEffect::Exit(args[0] as i32)),
            SYS_EVENTFD2 | SYS_EPOLL_CREATE1 | SYS_EPOLL_CTL | SYS_EPOLL_PWAIT | SYS_DUP3
            | SYS_FCNTL | SYS_IOCTL | SYS_READ | SYS_WRITE | SYS_OPENAT | SYS_CLOSE | SYS_PIPE2
            | SYS_LSEEK | SYS_PSELECT6 | SYS_PPOLL | SYS_READLINKAT | SYS_NEWFSTATAT
            | SYS_FSTAT | SYS_BRK | SYS_MMAP | SYS_MUNMAP | SYS_MPROTECT | SYS_MADVISE
            | SYS_RT_SIGACTION | SYS_RT_SIGPROCMASK | SYS_GETRANDOM => {
                bail!(
                    "syscall {} is on the allowlist but still needs guest memory marshalling in the executor",
                    number
                )
            }
            _ => bail!("unsupported syscall number {}", number),
        }
    }

    pub fn describe_allowlist(&self) -> Vec<&'static str> {
        vec![
            "read",
            "write",
            "eventfd2",
            "epoll_create1",
            "epoll_ctl",
            "epoll_pwait",
            "openat",
            "close",
            "lseek",
            "getcwd",
            "dup3",
            "fcntl",
            "ioctl",
            "readlinkat",
            "newfstatat",
            "fstat",
            "pipe2",
            "pselect6",
            "ppoll",
            "futex",
            "sigaltstack",
            "set_tid_address",
            "set_robust_list",
            "setuid",
            "setgid",
            "setresuid",
            "getresuid",
            "setresgid",
            "getresgid",
            "uname",
            "getppid",
            "wait4",
            "brk",
            "mmap",
            "munmap",
            "mprotect",
            "madvise",
            "prlimit64",
            "prctl",
            "rt_sigaction",
            "rt_sigprocmask",
            "clock_gettime",
            "getpid",
            "getuid",
            "geteuid",
            "getgid",
            "getegid",
            "gettid",
            "sysinfo",
            "getrandom",
            "membarrier",
            "rseq",
            "exit",
            "exit_group",
        ]
    }

    pub fn validate_env_strings(&self, config: &RuntimeConfig) -> Result<Vec<CString>> {
        config
            .env
            .iter()
            .map(|(k, v)| {
                CString::new(format!("{k}={v}")).context("invalid env contains interior NUL")
            })
            .collect()
    }
}

fn unsafe_pid(value: pid_t) -> u64 {
    value.max(0) as u64
}

fn host_clock_gettime(clk_id: libc::clockid_t) -> Result<timespec> {
    let mut ts = timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    let rc = unsafe { clock_gettime(clk_id, &mut ts as *mut timespec) };
    if rc != 0 {
        bail!(
            "clock_gettime({clk_id}) failed with errno {}",
            std::io::Error::last_os_error()
        );
    }
    Ok(ts)
}

fn build_boot_args(image: &LoadedElf, config: &RuntimeConfig) -> Vec<String> {
    let mut argv = Vec::with_capacity(config.args.len() + 1);
    argv.push(image.path.display().to_string());
    argv.extend(config.args.iter().cloned());
    argv
}

fn build_boot_env(config: &RuntimeConfig) -> Vec<String> {
    config.env.iter().map(|(k, v)| format!("{k}={v}")).collect()
}

fn initialize_user_stack(
    memory: &mut GuestMemory,
    stack_base: u64,
    stack_top: u64,
    argv: &[String],
    envp: &[String],
    entry: u64,
) -> Result<u64> {
    let mut sp = stack_top;

    let execfn_addr = push_c_string(
        memory,
        &mut sp,
        argv.first().map(String::as_str).unwrap_or(""),
    )?;
    let platform_addr = push_c_string(memory, &mut sp, "linx64")?;
    let random_addr = push_bytes(memory, &mut sp, &[0xA5; 16])?;

    let mut env_ptrs = Vec::with_capacity(envp.len());
    for item in envp.iter().rev() {
        env_ptrs.push(push_c_string(memory, &mut sp, item)?);
    }
    env_ptrs.reverse();

    let mut argv_ptrs = Vec::with_capacity(argv.len());
    for item in argv.iter().rev() {
        argv_ptrs.push(push_c_string(memory, &mut sp, item)?);
    }
    argv_ptrs.reverse();

    let auxv = vec![
        (AT_PAGESZ, PAGE_SIZE),
        (AT_ENTRY, entry),
        (AT_UID, 0),
        (AT_EUID, 0),
        (AT_GID, 0),
        (AT_EGID, 0),
        (AT_PLATFORM, platform_addr),
        (AT_HWCAP, 0),
        (AT_CLKTCK, 100),
        (AT_RANDOM, random_addr),
        (AT_HWCAP2, 0),
        (AT_HWCAP3, 0),
        (AT_HWCAP4, 0),
        (AT_EXECFN, execfn_addr),
        (AT_SYSINFO_EHDR, 0),
        (AT_MINSIGSTKSZ, GUEST_MINSIGSTKSZ),
        (AT_NULL, 0),
    ];

    let mut words = Vec::new();
    words.push(argv.len() as u64);
    words.extend(argv_ptrs.iter().copied());
    words.push(0);
    words.extend(env_ptrs.iter().copied());
    words.push(0);
    for (key, value) in auxv {
        words.push(key);
        words.push(value);
    }

    let frame_bytes = (words.len() * 8) as u64;
    sp = align_down(
        sp.checked_sub(frame_bytes)
            .context("guest stack underflow while writing argv/envp")?,
        16,
    );
    if sp < stack_base {
        bail!("guest stack initialization exceeds reserved stack region");
    }

    for (idx, word) in words.into_iter().enumerate() {
        memory
            .write_u64(sp + (idx as u64 * 8), word)
            .context("failed to populate initial stack words")?;
    }

    Ok(sp)
}

fn push_c_string(memory: &mut GuestMemory, sp: &mut u64, text: &str) -> Result<u64> {
    let mut bytes = text.as_bytes().to_vec();
    bytes.push(0);
    push_bytes(memory, sp, &bytes)
}

fn push_bytes(memory: &mut GuestMemory, sp: &mut u64, bytes: &[u8]) -> Result<u64> {
    let next_sp = sp
        .checked_sub(bytes.len() as u64)
        .context("guest stack underflow while writing bytes")?;
    memory
        .write_bytes(next_sp, bytes)
        .context("failed to write guest stack bytes")?;
    *sp = next_sp;
    Ok(next_sp)
}

pub fn guest_prot_to_region_flags(prot: u32) -> u32 {
    let mut flags = 0;
    if prot & libc::PROT_READ as u32 != 0 {
        flags |= MEM_READ;
    }
    if prot & libc::PROT_WRITE as u32 != 0 {
        flags |= MEM_WRITE;
    }
    if prot & libc::PROT_EXEC as u32 != 0 {
        flags |= MEM_EXEC;
    }
    flags
}

fn region_slice(region: &MemoryRegion, base: u64, end: u64, flags: u32) -> MemoryRegion {
    let start_off = (base - region.base) as usize;
    let end_off = (end - region.base) as usize;
    MemoryRegion {
        base,
        size: end - base,
        flags,
        data: region.data[start_off..end_off].to_vec(),
    }
}

fn align_down(value: u64, align: u64) -> u64 {
    debug_assert!(align.is_power_of_two());
    value & !(align - 1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use elf::SegmentImage;

    #[test]
    fn syscall_allowlist_has_getpid() {
        let shim = HostSyscallShim;
        let value = shim.dispatch(SYS_GETPID, [0; 6]).unwrap();
        assert!(matches!(value, SyscallEffect::Return(pid) if pid > 0));
    }

    #[test]
    fn bootstrap_sets_sp_and_argc() {
        let image = LoadedElf {
            path: PathBuf::from("sample.elf"),
            entry: 0x1000,
            little_endian: true,
            bits: 64,
            machine: 0xFEED,
            segments: vec![SegmentImage {
                vaddr: 0x1000,
                mem_size: 0x1000,
                file_size: 4,
                flags: 0b101,
                data: vec![0; 4],
            }],
        };
        let runtime = GuestRuntime::bootstrap(
            image,
            RuntimeConfig {
                args: vec!["arg1".to_string()],
                ..RuntimeConfig::default()
            },
        )
        .unwrap();

        assert_eq!(runtime.boot.argc, 2);
        assert_eq!(runtime.state.regs[1], runtime.boot.stack_pointer);
        assert_ne!(runtime.state.regs[1], runtime.boot.stack_top);
        assert_eq!(
            runtime.memory.read_u64(runtime.boot.stack_pointer).unwrap(),
            runtime.boot.argc
        );
    }

    #[test]
    fn bootstrap_populates_auxv_and_env() {
        let image = LoadedElf {
            path: PathBuf::from("/tmp/bootstrap.elf"),
            entry: 0x1000,
            little_endian: true,
            bits: 64,
            machine: 0xFEED,
            segments: vec![SegmentImage {
                vaddr: 0x1000,
                mem_size: 0x1000,
                file_size: 4,
                flags: 0b101,
                data: vec![0; 4],
            }],
        };
        let runtime = GuestRuntime::bootstrap(
            image,
            RuntimeConfig {
                args: vec!["alpha".to_string(), "beta".to_string()],
                env: BTreeMap::from([
                    ("LX_BOOTSTRAP".to_string(), "1".to_string()),
                    ("LX_TRACE".to_string(), "1".to_string()),
                ]),
                ..RuntimeConfig::default()
            },
        )
        .unwrap();

        let sp = runtime.boot.stack_pointer;
        assert_eq!(runtime.memory.read_u64(sp).unwrap(), 3);

        let argv0 = runtime.memory.read_u64(sp + 8).unwrap();
        let argv1 = runtime.memory.read_u64(sp + 16).unwrap();
        let argv2 = runtime.memory.read_u64(sp + 24).unwrap();
        assert_eq!(
            runtime.memory.read_c_string(argv0, 256).unwrap(),
            "/tmp/bootstrap.elf"
        );
        assert_eq!(runtime.memory.read_c_string(argv1, 256).unwrap(), "alpha");
        assert_eq!(runtime.memory.read_c_string(argv2, 256).unwrap(), "beta");

        let env0 = runtime.memory.read_u64(sp + 40).unwrap();
        let env1 = runtime.memory.read_u64(sp + 48).unwrap();
        assert_eq!(
            runtime.memory.read_c_string(env0, 256).unwrap(),
            "LX_BOOTSTRAP=1"
        );
        assert_eq!(
            runtime.memory.read_c_string(env1, 256).unwrap(),
            "LX_TRACE=1"
        );

        let aux_base = sp + 64;
        let mut aux = BTreeMap::new();
        let mut cursor = aux_base;
        loop {
            let key = runtime.memory.read_u64(cursor).unwrap();
            let value = runtime.memory.read_u64(cursor + 8).unwrap();
            aux.insert(key, value);
            cursor += 16;
            if key == AT_NULL {
                break;
            }
        }

        assert_eq!(aux.get(&AT_PAGESZ).copied().unwrap(), PAGE_SIZE);
        assert_eq!(aux.get(&AT_ENTRY).copied().unwrap(), 0x1000);
        assert_eq!(aux.get(&AT_PLATFORM).copied().unwrap(), aux[&AT_PLATFORM]);
        assert_eq!(aux.get(&AT_HWCAP).copied().unwrap(), 0);
        assert_eq!(aux.get(&AT_CLKTCK).copied().unwrap(), 100);
        assert_eq!(aux.get(&AT_HWCAP2).copied().unwrap(), 0);
        assert_eq!(aux.get(&AT_HWCAP3).copied().unwrap(), 0);
        assert_eq!(aux.get(&AT_HWCAP4).copied().unwrap(), 0);
        assert_eq!(aux.get(&AT_SYSINFO_EHDR).copied().unwrap(), 0);
        assert_eq!(
            aux.get(&AT_MINSIGSTKSZ).copied().unwrap(),
            GUEST_MINSIGSTKSZ
        );

        let platform_addr = aux[&AT_PLATFORM];
        let random_addr = aux[&AT_RANDOM];
        let execfn_addr = aux[&AT_EXECFN];
        let random = runtime.memory.read_bytes(random_addr, 16).unwrap();
        assert!(random.iter().all(|byte| *byte == 0xA5));
        assert_eq!(
            runtime.memory.read_c_string(platform_addr, 32).unwrap(),
            "linx64"
        );
        assert_eq!(
            runtime.memory.read_c_string(execfn_addr, 256).unwrap(),
            "/tmp/bootstrap.elf"
        );
    }

    #[test]
    fn protect_range_splits_region_and_enforces_permissions() {
        let mut memory = GuestMemory {
            regions: vec![MemoryRegion {
                base: 0x4000,
                size: PAGE_SIZE * 2,
                flags: MEM_READ | MEM_WRITE,
                data: vec![0xAA; (PAGE_SIZE * 2) as usize],
            }],
        };

        assert!(memory.protect_range(0x5000, PAGE_SIZE, MEM_READ));
        assert_eq!(memory.regions.len(), 2);
        assert_eq!(memory.regions[0].base, 0x4000);
        assert_eq!(memory.regions[0].flags, MEM_READ | MEM_WRITE);
        assert_eq!(memory.regions[1].base, 0x5000);
        assert_eq!(memory.regions[1].flags, MEM_READ);
        assert_eq!(memory.read_u8_checked(0x5000), Some(0xAA));
        assert!(memory.write_bytes_checked(0x5000, &[0x55]).is_none());
        assert_eq!(memory.write_bytes_checked(0x4000, &[0x55]), Some(()));
    }

    #[test]
    fn unmap_range_removes_overlap_only() {
        let mut memory = GuestMemory {
            regions: vec![MemoryRegion {
                base: 0x4000,
                size: PAGE_SIZE * 3,
                flags: MEM_READ | MEM_WRITE,
                data: vec![0x11; (PAGE_SIZE * 3) as usize],
            }],
        };

        memory.unmap_range(0x5000, PAGE_SIZE);
        assert_eq!(memory.regions.len(), 2);
        assert_eq!(memory.regions[0].base, 0x4000);
        assert_eq!(memory.regions[0].size, PAGE_SIZE);
        assert_eq!(memory.regions[1].base, 0x6000);
        assert_eq!(memory.regions[1].size, PAGE_SIZE);
        assert_eq!(memory.read_u8(0x4000), Some(0x11));
        assert!(memory.read_u8(0x5000).is_none());
        assert_eq!(memory.read_u8(0x6000), Some(0x11));
    }
}
