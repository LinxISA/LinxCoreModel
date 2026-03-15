typedef unsigned int u32;
typedef unsigned long u64;

struct stat_guest {
    u64 dev;
    u64 ino;
    u32 mode;
    u32 nlink;
    u32 uid;
    u32 gid;
    u64 rdev;
    u64 __pad0;
    long size;
    int blksize;
    int __pad1;
    long blocks;
    long atime_sec;
    u64 atime_nsec;
    long mtime_sec;
    u64 mtime_nsec;
    long ctime_sec;
    u64 ctime_nsec;
    u32 __unused[3];
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

static inline __attribute__((noreturn)) void linx_exit(int code)
{
    (void)linx_syscall1(93, code);
    __builtin_unreachable();
}

__attribute__((noreturn)) void _start(void)
{
    static const char ok[] = "path random ok\n";
    static const char file_name[] = "file_io_input.txt";
    char cwd[256];
    unsigned char random_buf[16];
    struct stat_guest st;
    long fd;
    long flags;
    long count;
    int i;

    if (linx_syscall2(17, (long)cwd, (long)sizeof(cwd)) != (long)cwd)
        linx_exit(130);
    if (cwd[0] != '/')
        linx_exit(131);

    if (linx_syscall4(79, -100, (long)file_name, (long)&st, 0) != 0)
        linx_exit(132);
    if (st.size != 27)
        linx_exit(133);

    fd = linx_syscall4(56, -100, (long)file_name, 0, 0);
    if (fd < 0)
        linx_exit(134);

    if (linx_syscall3(25, fd, 2, 1) != 0)
        linx_exit(135);
    flags = linx_syscall2(25, fd, 1);
    if (flags != 1)
        linx_exit(136);

    count = linx_syscall3(278, (long)random_buf, (long)sizeof(random_buf), 0);
    if (count != (long)sizeof(random_buf))
        linx_exit(137);
    for (i = 0; i < (int)sizeof(random_buf); ++i) {
        if (random_buf[i] != 0)
            break;
    }
    if (i == (int)sizeof(random_buf))
        linx_exit(138);

    if (linx_syscall3(64, 1, (long)ok, (long)(sizeof(ok) - 1)) != (long)(sizeof(ok) - 1))
        linx_exit(139);

    if (linx_syscall1(57, fd) != 0)
        linx_exit(140);

    linx_exit(0);
}
