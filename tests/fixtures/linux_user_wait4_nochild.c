typedef unsigned long u64;
typedef unsigned int u32;

struct timeval_guest {
    long tv_sec;
    long tv_usec;
};

struct rusage_guest {
    struct timeval_guest ru_utime;
    struct timeval_guest ru_stime;
    long fields[14 + 16];
};

void *memset(void *dst, int value, unsigned long count)
{
    unsigned char *ptr = (unsigned char *)dst;
    unsigned char byte = (unsigned char)value;
    unsigned long i;
    for (i = 0; i < count; ++i)
        ptr[i] = byte;
    return dst;
}

static inline long linx_syscall1(long number, long arg0)
{
    register long a7 __asm__("a7") = number;
    register long a0 __asm__("a0") = arg0;
    __asm__ volatile("acrc 1" : "+r"(a0) : "r"(a7) : "memory");
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
    static const char ok[] = "wait4 nochild ok\n";
    int status = 0x11223344;
    struct rusage_guest ru;
    long rc;
    memset(&ru, 0xA5, sizeof(ru));

    rc = linx_syscall4(260, -1, (long)&status, 0, (long)&ru);
    if (rc != -10)
        linx_exit(230);
    if (status != 0x11223344)
        linx_exit(231);

    if (linx_syscall3(64, 1, (long)ok, (long)(sizeof(ok) - 1)) != (long)(sizeof(ok) - 1))
        linx_exit(232);

    linx_exit(0);
}
