typedef unsigned long u64;
typedef unsigned int u32;

struct pollfd_guest {
    int fd;
    short events;
    short revents;
};

struct timespec_guest {
    long tv_sec;
    long tv_nsec;
};

struct stack_t_guest {
    void *ss_sp;
    int ss_flags;
    int __pad;
    unsigned long ss_size;
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

__attribute__((noreturn)) void _start(void)
{
    static const char ok[] = "ppoll sigaltstack ok\n";
    char alt[4096];
    char in[2];
    char out[2] = {'o', 'k'};
    int pipefd[2];
    struct stack_t_guest ss;
    struct stack_t_guest old;
    struct pollfd_guest pfd;
    struct timespec_guest ts;

    ss.ss_sp = alt;
    ss.ss_flags = 0;
    ss.__pad = 0;
    ss.ss_size = sizeof(alt);
    old.ss_sp = 0;
    old.ss_flags = 0;
    old.__pad = 0;
    old.ss_size = 0;

    if (linx_syscall2(132, (long)&ss, (long)&old) != 0)
        linx_exit(190);
    if (old.ss_flags != 2)
        linx_exit(191);

    old.ss_sp = 0;
    old.ss_flags = 0;
    old.ss_size = 0;
    if (linx_syscall2(132, 0, (long)&old) != 0)
        linx_exit(192);

    if (linx_syscall2(59, (long)pipefd, 0) != 0)
        linx_exit(193);
    if (linx_syscall3(64, pipefd[1], (long)out, 2) != 2)
        linx_exit(194);

    pfd.fd = pipefd[0];
    pfd.events = 0x001;
    pfd.revents = 0;
    ts.tv_sec = 0;
    ts.tv_nsec = 0;
    if (linx_syscall5(73, (long)&pfd, 1, (long)&ts, 0, 0) != 1)
        linx_exit(195);
    if ((pfd.revents & 0x001) == 0)
        linx_exit(196);

    if (linx_syscall3(63, pipefd[0], (long)in, 2) != 2)
        linx_exit(197);
    if (in[0] != 'o' || in[1] != 'k')
        linx_exit(198);

    if (linx_syscall3(64, 1, (long)ok, (long)(sizeof(ok) - 1)) != (long)(sizeof(ok) - 1))
        linx_exit(199);

    if (linx_syscall1(57, pipefd[1]) != 0)
        linx_exit(200);
    if (linx_syscall1(57, pipefd[0]) != 0)
        linx_exit(201);

    linx_exit(0);
}
