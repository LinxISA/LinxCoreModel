typedef unsigned long u64;

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

static inline long linx_syscall5(long number, long arg0, long arg1, long arg2, long arg3, long arg4)
{
    register long a7 __asm__("a7") = number;
    register long a0 __asm__("a0") = arg0;
    register long a1 __asm__("a1") = arg1;
    register long a2 __asm__("a2") = arg2;
    register long a3 __asm__("a3") = arg3;
    register long a4 __asm__("a4") = arg4;
    __asm__ volatile("acrc 1" : "+r"(a0) : "r"(a7), "r"(a1), "r"(a2), "r"(a3), "r"(a4) : "memory");
    return a0;
}

static inline __attribute__((noreturn)) void linx_exit(int code)
{
    (void)linx_syscall1(93, code);
    __builtin_unreachable();
}

static int str_eq(const char *lhs, const char *rhs)
{
    while (*lhs && *rhs) {
        if (*lhs != *rhs)
            return 0;
        ++lhs;
        ++rhs;
    }
    return *lhs == *rhs;
}

__attribute__((noreturn)) void _start(void)
{
    static const char ok[] = "thread runtime ok\n";
    static const char name[] = "bringup-thr";
    char got_name[16];
    unsigned char *page;
    long query;

    page = (unsigned char *)linx_syscall5(222, 0, 4096, 3, 0x22, -1);
    if ((long)page < 0)
        linx_exit(150);
    page[0] = 0x5a;

    if (linx_syscall5(167, 15, (long)name, 0, 0, 0) != 0)
        linx_exit(151);
    if (linx_syscall5(167, 16, (long)got_name, 0, 0, 0) != 0)
        linx_exit(152);
    if (!str_eq(got_name, "bringup-thr"))
        linx_exit(153);

    if (linx_syscall3(233, (long)page, 4096, 4) != 0)
        linx_exit(154);

    query = linx_syscall2(283, 0, 0);
    if ((query & 8) == 0 || (query & 16) == 0)
        linx_exit(155);
    if (linx_syscall2(283, 16, 0) != 0)
        linx_exit(156);
    if (linx_syscall2(283, 8, 0) != 0)
        linx_exit(157);

    if (linx_syscall3(64, 1, (long)ok, (long)(sizeof(ok) - 1)) != (long)(sizeof(ok) - 1))
        linx_exit(158);

    linx_exit(0);
}
