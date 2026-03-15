typedef unsigned long u64;
typedef unsigned int u32;

struct epoll_event_guest {
    u32 events;
    u32 __pad;
    u64 data;
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

static inline long linx_syscall6(long number, long arg0, long arg1, long arg2, long arg3, long arg4, long arg5)
{
    register long a7 __asm__("a7") = number;
    register long a0 __asm__("a0") = arg0;
    register long a1 __asm__("a1") = arg1;
    register long a2 __asm__("a2") = arg2;
    register long a3 __asm__("a3") = arg3;
    register long a4 __asm__("a4") = arg4;
    register long a5 __asm__("a5") = arg5;
    __asm__ volatile("acrc 1" : "+r"(a0) : "r"(a7), "r"(a1), "r"(a2), "r"(a3), "r"(a4), "r"(a5) : "memory");
    return a0;
}

static inline __attribute__((noreturn)) void linx_exit(int code)
{
    (void)linx_syscall1(93, code);
    __builtin_unreachable();
}

__attribute__((noreturn)) void _start(void)
{
    static const char ok[] = "epoll eventfd ok\n";
    u64 write_value = 7;
    u64 read_value = 0;
    int event_fd;
    int epoll_fd;
    struct epoll_event_guest ctl;
    struct epoll_event_guest out[2];

    ctl.events = 0x001;
    ctl.__pad = 0;
    ctl.data = 0x1122334455667788UL;
    out[0].events = 0;
    out[0].__pad = 0;
    out[0].data = 0;
    out[1].events = 0;
    out[1].__pad = 0;
    out[1].data = 0;

    event_fd = (int)linx_syscall2(19, 0, 0);
    if (event_fd < 0)
        linx_exit(210);

    epoll_fd = (int)linx_syscall1(20, 0);
    if (epoll_fd < 0)
        linx_exit(211);

    if (linx_syscall4(21, epoll_fd, 1, event_fd, (long)&ctl) != 0)
        linx_exit(212);
    if (linx_syscall3(64, event_fd, (long)&write_value, 8) != 8)
        linx_exit(213);
    if (linx_syscall6(22, epoll_fd, (long)out, 2, 0, 0, 0) != 1)
        linx_exit(214);
    if ((out[0].events & 0x001) == 0)
        linx_exit(215);
    if (out[0].data != 0x1122334455667788UL)
        linx_exit(216);
    if (linx_syscall3(63, event_fd, (long)&read_value, 8) != 8)
        linx_exit(217);
    if (read_value != 7)
        linx_exit(218);
    if (linx_syscall3(64, 1, (long)ok, (long)(sizeof(ok) - 1)) != (long)(sizeof(ok) - 1))
        linx_exit(219);
    if (linx_syscall1(57, epoll_fd) != 0)
        linx_exit(220);
    if (linx_syscall1(57, event_fd) != 0)
        linx_exit(221);

    linx_exit(0);
}
