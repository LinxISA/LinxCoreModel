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

struct linx_stat {
    unsigned long st_dev;
    unsigned long st_ino;
    unsigned int st_mode;
    unsigned int st_nlink;
    unsigned int st_uid;
    unsigned int st_gid;
    unsigned long st_rdev;
    unsigned long __pad1;
    long st_size;
    int st_blksize;
    int __pad2;
    long st_blocks;
    long st_atime;
    unsigned long st_atime_nsec;
    long st_mtime;
    unsigned long st_mtime_nsec;
    long st_ctime;
    unsigned long st_ctime_nsec;
    unsigned int __unused4;
    unsigned int __unused5;
};

struct linx_timespec {
    long tv_sec;
    long tv_nsec;
};

typedef char linx_stat_size_is_128[(sizeof(struct linx_stat) == 128) ? 1 : -1];

static long buffer_matches(const char *lhs, const char *rhs, long len)
{
    long idx = 0;
    while (idx < len) {
        if (lhs[idx] != rhs[idx])
            return 0;
        idx += 1;
    }
    return 1;
}

__attribute__((noreturn)) void _start(void)
{
    static const char path[] = "file_io_input.txt";
    static const char expected[] = "fixture-data:linxcoremodel\n";
    static const char ok[] = "stat lseek ok\n";
    static const char bad[] = "stat lseek bad\n";
    char buffer[32];
    struct linx_stat st;
    struct linx_timespec ts;
    long fd = linx_syscall4(56, -100, (long)path, 0, 0);
    long size;
    long end;
    long wrote;

    if (fd < 0)
        linx_exit(20);

    if (linx_syscall2(80, fd, (long)&st) < 0)
        linx_exit(21);

    if (st.st_size != (long)(sizeof(expected) - 1))
        linx_exit(22);

    end = linx_syscall3(62, fd, 0, 2);
    if (end != st.st_size)
        linx_exit(23);

    if (linx_syscall3(62, fd, 0, 0) != 0)
        linx_exit(24);

    if (linx_syscall2(113, 0, (long)&ts) < 0)
        linx_exit(25);

    if (ts.tv_nsec < 0 || ts.tv_nsec >= 1000000000L)
        linx_exit(26);

    size = linx_syscall3(63, fd, (long)buffer, (long)(sizeof(expected) - 1));
    if (size != st.st_size)
        linx_exit(27);

    (void)linx_syscall1(57, fd);

    if (!buffer_matches(buffer, expected, size))
        linx_exit(28);

    wrote = linx_syscall3(64, 1, (long)ok, (long)(sizeof(ok) - 1));
    if (wrote < 0)
        linx_exit(29);

    linx_exit(0);
}
