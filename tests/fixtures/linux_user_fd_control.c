typedef unsigned int u32;

struct winsize_guest {
    unsigned short row;
    unsigned short col;
    unsigned short xpixel;
    unsigned short ypixel;
};

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

__attribute__((noreturn)) void _start(void)
{
    static const char ok[] = "fd control ok\n";
    int pipefd[2];
    int read_fd;
    int dup_fd;
    char in[3];
    struct winsize_guest ws;
    u32 pgrp = 77;

    if (linx_syscall2(59, (long)pipefd, 02000000) != 0)
        linx_exit(170);
    read_fd = pipefd[0];
    dup_fd = (int)linx_syscall3(24, pipefd[1], 20, 02000000);
    if (dup_fd != 20)
        linx_exit(171);
    if (linx_syscall2(25, dup_fd, 1) != 1)
        linx_exit(172);

    if (linx_syscall3(64, dup_fd, (long)"abc", 3) != 3)
        linx_exit(173);
    if (linx_syscall3(63, read_fd, (long)in, 3) != 3)
        linx_exit(174);
    if (in[0] != 'a' || in[1] != 'b' || in[2] != 'c')
        linx_exit(175);

    if (linx_syscall3(29, 1, 0x5413, (long)&ws) != 0)
        linx_exit(176);
    if (ws.row != 24 || ws.col != 80)
        linx_exit(177);

    if (linx_syscall3(29, 1, 0x5410, (long)&pgrp) != 0)
        linx_exit(178);
    pgrp = 0;
    if (linx_syscall3(29, 1, 0x540f, (long)&pgrp) != 0)
        linx_exit(179);
    if (pgrp != 77)
        linx_exit(180);

    if (linx_syscall3(64, 1, (long)ok, (long)(sizeof(ok) - 1)) != (long)(sizeof(ok) - 1))
        linx_exit(181);

    if (linx_syscall1(57, dup_fd) != 0)
        linx_exit(182);
    if (linx_syscall1(57, pipefd[1]) != 0)
        linx_exit(183);
    if (linx_syscall1(57, read_fd) != 0)
        linx_exit(184);

    linx_exit(0);
}
