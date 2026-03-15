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
    static const char ok[] = "file io ok\n";
    static const char bad[] = "file io bad\n";
    char buffer[32];
    long fd = linx_syscall4(56, -100, (long)path, 0, 0);
    long size;
    long wrote;

    if (fd < 0)
        linx_exit(10);

    size = linx_syscall3(63, fd, (long)buffer, (long)(sizeof(expected) - 1));
    if (size < 0)
        linx_exit(11);

    (void)linx_syscall1(57, fd);

    if (size == (long)(sizeof(expected) - 1) &&
        buffer_matches(buffer, expected, size)) {
        wrote = linx_syscall3(64, 1, (long)ok, (long)(sizeof(ok) - 1));
        if (wrote < 0)
            linx_exit(12);
        linx_exit(0);
    }

    wrote = linx_syscall3(64, 1, (long)bad, (long)(sizeof(bad) - 1));
    if (wrote < 0)
        linx_exit(13);
    linx_exit(1);
}
