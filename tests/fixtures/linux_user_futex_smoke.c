typedef unsigned int u32;

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

static inline long linx_syscall1(long number, long arg0)
{
    register long a7 __asm__("a7") = number;
    register long a0 __asm__("a0") = arg0;
    __asm__ volatile("acrc 1" : "+r"(a0) : "r"(a7) : "memory");
    return a0;
}

static inline __attribute__((noreturn)) void linx_exit(int code)
{
    (void)linx_syscall1(93, code);
    __builtin_unreachable();
}

#define FUTEX_WAIT 0
#define FUTEX_WAKE 1
#define FUTEX_PRIVATE 128
#define EAGAIN 11L

__attribute__((noreturn)) void _start(void)
{
    static const char ok[] = "futex smoke ok\n";
    u32 fut = 7;
    long rc;

    rc = linx_syscall4(98, (long)&fut, FUTEX_WAIT | FUTEX_PRIVATE, 3, 0);
    if (rc != -EAGAIN)
        linx_exit(80);

    rc = linx_syscall3(98, (long)&fut, FUTEX_WAKE | FUTEX_PRIVATE, 1);
    if (rc != 0)
        linx_exit(81);

    fut = 2;
    fut = 5;
    rc = linx_syscall4(98, (long)&fut, FUTEX_WAIT | FUTEX_PRIVATE, 2, 0);
    if (rc != -EAGAIN)
        linx_exit(82);

    if (linx_syscall3(64, 1, (long)ok, (long)(sizeof(ok) - 1)) != (long)(sizeof(ok) - 1))
        linx_exit(83);

    linx_exit(0);
}
