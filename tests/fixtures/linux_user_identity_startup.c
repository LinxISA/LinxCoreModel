typedef unsigned long u64;

static inline long linx_syscall0(long number)
{
    register long a7 __asm__("a7") = number;
    register long a0 __asm__("a0");
    __asm__ volatile("acrc 1" : "=r"(a0) : "r"(a7) : "memory");
    return a0;
}

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

static inline __attribute__((noreturn)) void linx_exit(int code)
{
    (void)linx_syscall1(93, code);
    __builtin_unreachable();
}

struct utsname_guest {
    char sysname[65];
    char nodename[65];
    char release[65];
    char version[65];
    char machine[65];
    char domainname[65];
};

struct robust_list_head_guest {
    u64 next;
    long futex_offset;
    u64 pending;
};

static int string_eq(const char *lhs, const char *rhs)
{
    while (*lhs != 0 && *rhs != 0) {
        if (*lhs != *rhs)
            return 0;
        ++lhs;
        ++rhs;
    }
    return *lhs == *rhs;
}

__attribute__((noreturn)) void _start(void)
{
    static const char ok[] = "identity startup ok\n";
    struct utsname_guest uts;
    struct robust_list_head_guest robust = {0, 0, 0};
    int clear_child_tid = -1;
    long tid;

    tid = linx_syscall1(96, (long)&clear_child_tid);
    if (tid <= 0)
        linx_exit(70);
    if (tid != linx_syscall0(178))
        linx_exit(71);

    if (linx_syscall2(99, (long)&robust, (long)sizeof(robust)) != 0)
        linx_exit(72);

    if (linx_syscall1(160, (long)&uts) != 0)
        linx_exit(73);
    if (!string_eq(uts.sysname, "Linux"))
        linx_exit(74);
    if (!string_eq(uts.machine, "linx64"))
        linx_exit(75);
    if (!string_eq(uts.nodename, "linxcoremodel"))
        linx_exit(76);

    if (linx_syscall0(174) != 0 || linx_syscall0(175) != 0)
        linx_exit(77);
    if (linx_syscall0(176) != 0 || linx_syscall0(177) != 0)
        linx_exit(78);

    if (linx_syscall3(64, 1, (long)ok, (long)(sizeof(ok) - 1)) != (long)(sizeof(ok) - 1))
        linx_exit(79);

    linx_exit(0);
}
