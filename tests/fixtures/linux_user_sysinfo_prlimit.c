typedef unsigned long u64;
typedef unsigned int u32;

static inline long linx_syscall1(long number, long arg0)
{
    register long a7 __asm__("a7") = number;
    register long a0 __asm__("a0") = arg0;
    __asm__ volatile("acrc 1" : "+r"(a0) : "r"(a7) : "memory");
    return a0;
}

static inline long linx_syscall2(long number, long arg0, long arg1)
{
    register long a7 __asm__("a7") = number;
    register long a0 __asm__("a0") = arg0;
    register long a1 __asm__("a1") = arg1;
    __asm__ volatile("acrc 1" : "+r"(a0) : "r"(a7), "r"(a1) : "memory");
    return a0;
}

static inline long linx_syscall3(long number, long arg0, long arg1, long arg2)
{
    register long a7 __asm__("a7") = number;
    register long a0 __asm__("a0") = arg0;
    register long a1 __asm__("a1") = arg1;
    register long a2 __asm__("a2") = arg2;
    __asm__ volatile("acrc 1" : "+r"(a0) : "r"(a7), "r"(a1), "r"(a2) : "memory");
    return a0;
}

static inline long linx_syscall4(long number, long arg0, long arg1, long arg2, long arg3)
{
    register long a7 __asm__("a7") = number;
    register long a0 __asm__("a0") = arg0;
    register long a1 __asm__("a1") = arg1;
    register long a2 __asm__("a2") = arg2;
    register long a3 __asm__("a3") = arg3;
    __asm__ volatile("acrc 1" : "+r"(a0) : "r"(a7), "r"(a1), "r"(a2), "r"(a3) : "memory");
    return a0;
}

static inline __attribute__((noreturn)) void linx_exit(int code)
{
    (void)linx_syscall1(93, code);
    __builtin_unreachable();
}

struct sysinfo_guest {
    u64 uptime;
    u64 loads[3];
    u64 totalram;
    u64 freeram;
    u64 sharedram;
    u64 bufferram;
    u64 totalswap;
    u64 freeswap;
    unsigned short procs;
    unsigned short pad;
    u64 totalhigh;
    u64 freehigh;
    u32 mem_unit;
    char reserved[256];
};

struct rlimit_guest {
    u64 cur;
    u64 max;
};

#define RLIMIT_STACK 3

__attribute__((noreturn)) void _start(void)
{
    static const char ok[] = "sysinfo prlimit ok\n";
    struct sysinfo_guest si;
    struct rlimit_guest lim;
    struct rlimit_guest new_lim;
    long ppid;

    ppid = linx_syscall1(173, 0);
    if (ppid <= 0)
        linx_exit(90);

    if (linx_syscall1(179, (long)&si) != 0)
        linx_exit(91);
    if (si.totalram == 0 || si.mem_unit == 0)
        linx_exit(92);
    if (si.freeram > si.totalram)
        linx_exit(93);
    if (si.procs != 1)
        linx_exit(94);

    if (linx_syscall4(261, 0, RLIMIT_STACK, 0, (long)&lim) != 0)
        linx_exit(95);
    if (lim.cur == 0 || lim.max == 0 || lim.cur > lim.max)
        linx_exit(96);

    new_lim.cur = lim.cur / 2;
    if (new_lim.cur == 0)
        linx_exit(97);
    new_lim.max = lim.max;

    if (linx_syscall4(261, 0, RLIMIT_STACK, (long)&new_lim, 0) != 0)
        linx_exit(98);
    if (linx_syscall4(261, 0, RLIMIT_STACK, 0, (long)&lim) != 0)
        linx_exit(99);
    if (lim.cur != new_lim.cur || lim.max != new_lim.max)
        linx_exit(100);

    if (linx_syscall3(64, 1, (long)ok, (long)(sizeof(ok) - 1)) != (long)(sizeof(ok) - 1))
        linx_exit(101);

    linx_exit(0);
}
