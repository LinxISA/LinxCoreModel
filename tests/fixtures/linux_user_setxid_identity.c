typedef unsigned int u32;

static inline long linx_syscall1(long number, long arg0)
{
    register long a7 __asm__("a7") = number;
    register long a0 __asm__("a0") = arg0;
    __asm__ volatile("acrc 1" : "+r"(a0) : "r"(a7) : "memory");
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

__attribute__((noreturn)) void _start(void)
{
    static const char ok[] = "setxid identity ok\n";
    u32 ruid = 99;
    u32 euid = 99;
    u32 suid = 99;
    u32 rgid = 77;
    u32 egid = 77;
    u32 sgid = 77;

    if (linx_syscall3(148, (long)&ruid, (long)&euid, (long)&suid) != 0)
        linx_exit(110);
    if (linx_syscall3(150, (long)&rgid, (long)&egid, (long)&sgid) != 0)
        linx_exit(111);
    if (ruid != 0 || euid != 0 || suid != 0)
        linx_exit(112);
    if (rgid != 0 || egid != 0 || sgid != 0)
        linx_exit(113);

    if (linx_syscall1(146, 0) != 0)
        linx_exit(114);
    if (linx_syscall1(144, 0) != 0)
        linx_exit(115);
    if (linx_syscall3(147, -1, 0, -1) != 0)
        linx_exit(116);
    if (linx_syscall3(149, -1, 0, -1) != 0)
        linx_exit(117);

    ruid = euid = suid = 99;
    rgid = egid = sgid = 77;
    if (linx_syscall3(148, (long)&ruid, (long)&euid, (long)&suid) != 0)
        linx_exit(118);
    if (linx_syscall3(150, (long)&rgid, (long)&egid, (long)&sgid) != 0)
        linx_exit(119);
    if (ruid != 0 || euid != 0 || suid != 0)
        linx_exit(120);
    if (rgid != 0 || egid != 0 || sgid != 0)
        linx_exit(121);

    if (linx_syscall3(64, 1, (long)ok, (long)(sizeof(ok) - 1)) != (long)(sizeof(ok) - 1))
        linx_exit(122);

    linx_exit(0);
}
