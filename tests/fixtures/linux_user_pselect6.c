typedef unsigned long u64;

struct timespec_guest {
    long tv_sec;
    long tv_nsec;
};

struct pselect_sigdata {
    const void *sigmask;
    u64 sigset_size;
};

struct fd_set_guest {
    unsigned long fds_bits[16];
};

void *memset(void *dst, int value, unsigned long count)
{
    unsigned char *ptr = (unsigned char *)dst;
    unsigned long idx;
    for (idx = 0; idx < count; ++idx)
        ptr[idx] = (unsigned char)value;
    return dst;
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

static void fd_zero(struct fd_set_guest *set)
{
    int i;
    for (i = 0; i < 16; ++i)
        set->fds_bits[i] = 0;
}

static void fd_set_one(int fd, struct fd_set_guest *set)
{
    set->fds_bits[fd / 64] |= 1UL << (fd % 64);
}

static int fd_isset(int fd, const struct fd_set_guest *set)
{
    return (set->fds_bits[fd / 64] & (1UL << (fd % 64))) != 0;
}

__attribute__((noreturn)) void _start(void)
{
    static const char ok[] = "pselect6 ok\n";
    char out[2] = {'o', 'k'};
    char in[2];
    int pipefd[2];
    struct fd_set_guest readfds;
    struct timespec_guest ts;
    struct pselect_sigdata sigdata;
    int i;

    in[0] = 0;
    in[1] = 0;
    pipefd[0] = 0;
    pipefd[1] = 0;
    for (i = 0; i < 16; ++i)
        readfds.fds_bits[i] = 0;

    if (linx_syscall2(59, (long)pipefd, 0) != 0)
        linx_exit(210);
    if (linx_syscall3(64, pipefd[1], (long)out, 2) != 2)
        linx_exit(211);

    fd_set_one(pipefd[0], &readfds);
    ts.tv_sec = 0;
    ts.tv_nsec = 0;
    sigdata.sigmask = 0;
    sigdata.sigset_size = 0;

    if (linx_syscall6(72, pipefd[0] + 1, (long)&readfds, 0, 0, (long)&ts, (long)&sigdata) != 1)
        linx_exit(212);
    if (!fd_isset(pipefd[0], &readfds))
        linx_exit(213);

    if (linx_syscall3(63, pipefd[0], (long)in, 2) != 2)
        linx_exit(214);
    if (in[0] != 'o' || in[1] != 'k')
        linx_exit(215);

    if (linx_syscall3(64, 1, (long)ok, (long)(sizeof(ok) - 1)) != (long)(sizeof(ok) - 1))
        linx_exit(216);
    if (linx_syscall1(57, pipefd[1]) != 0)
        linx_exit(217);
    if (linx_syscall1(57, pipefd[0]) != 0)
        linx_exit(218);

    linx_exit(0);
}
