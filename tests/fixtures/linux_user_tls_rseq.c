typedef unsigned int u32;
typedef unsigned long u64;

struct rseq_guest {
    u32 cpu_id_start;
    u32 cpu_id;
    u64 rseq_cs;
    u32 flags;
    u32 pad;
    u64 extra[1];
};

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

static inline void linx_set_tp(void *p)
{
    __asm__ volatile("ssrset %0, %1" : : "r"(p), "i"(0x0000) : "memory");
}

static inline void *linx_get_tp(void)
{
    void *tp;
    __asm__ volatile("ssrget %1, ->%0" : "=r"(tp) : "i"(0x0000) : "memory");
    return tp;
}

__attribute__((noreturn)) void _start(void)
{
    static const char ok[] = "tls rseq ok\n";
    struct rseq_guest rseq;
    unsigned long tp_cookie = 0x123456789abcdef0ul;

    linx_set_tp(&tp_cookie);
    if (linx_get_tp() != &tp_cookie)
        linx_exit(160);

    if (linx_syscall4(293, (long)&rseq, (long)sizeof(rseq), 0, 0x53053053) != 0)
        linx_exit(161);
    if (rseq.cpu_id_start != 0 || rseq.cpu_id != 0 || rseq.rseq_cs != 0 || rseq.flags != 0)
        linx_exit(162);

    if (linx_syscall4(293, (long)&rseq, (long)sizeof(rseq), 1, 0x53053053) != 0)
        linx_exit(163);

    if (linx_syscall3(64, 1, (long)ok, (long)(sizeof(ok) - 1)) != (long)(sizeof(ok) - 1))
        linx_exit(164);

    linx_exit(0);
}
