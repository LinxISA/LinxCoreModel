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
    register long a7 __asm__("a7") = 93;
    register long a0 __asm__("a0") = code;
    __asm__ volatile("acrc 1" : "+r"(a0) : "r"(a7) : "memory");
    __builtin_unreachable();
}

static long mix_values(long x, long y)
{
    long acc = x + y;
    if ((acc & 1) == 0)
        return acc + 7;
    return acc - 3;
}

__attribute__((noreturn)) void _start(void)
{
    static const char ok[] = "compiler path ok\n";
    static const char bad[] = "compiler path bad\n";
    long value = mix_values(10, 12);
    const char *msg = ok;
    long len = (long)(sizeof(ok) - 1);

    if (value != 29) {
        msg = bad;
        len = (long)(sizeof(bad) - 1);
    }

    (void)linx_syscall3(64, 1, (long)msg, len);
    linx_exit(value == 29 ? 0 : 1);
}
