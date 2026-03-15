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

static inline long linx_syscall6(
    long number,
    long arg0,
    long arg1,
    long arg2,
    long arg3,
    long arg4,
    long arg5)
{
    register long a7 __asm__("a7") = number;
    register long a0 __asm__("a0") = arg0;
    register long a1 __asm__("a1") = arg1;
    register long a2 __asm__("a2") = arg2;
    register long a3 __asm__("a3") = arg3;
    register long a4 __asm__("a4") = arg4;
    register long a5 __asm__("a5") = arg5;
    __asm__ volatile(
        "acrc 1"
        : "+r"(a0)
        : "r"(a7), "r"(a1), "r"(a2), "r"(a3), "r"(a4), "r"(a5)
        : "memory");
    return a0;
}

static inline __attribute__((noreturn)) void linx_exit(int code)
{
    (void)linx_syscall1(93, code);
    __builtin_unreachable();
}

#define PROT_NONE 0x0
#define PROT_READ 0x1
#define PROT_WRITE 0x2
#define MAP_PRIVATE 0x02
#define MAP_ANONYMOUS 0x20
#define PAGE_SIZE 4096L
#define EFAULT 14L

__attribute__((noreturn)) void _start(void)
{
    static const char ok[] = "mprotect signal ok\n";
    unsigned long old_mask[2] = {~0UL, ~0UL};
    char *map = (char *)linx_syscall6(
        222,
        0,
        PAGE_SIZE,
        PROT_READ | PROT_WRITE,
        MAP_PRIVATE | MAP_ANONYMOUS,
        -1,
        0);
    long rc;

    if ((long)map < 0)
        linx_exit(60);

    map[0] = 'P';

    if (linx_syscall3(226, (long)map, PAGE_SIZE, PROT_NONE) != 0)
        linx_exit(61);

    rc = linx_syscall3(64, 1, (long)map, 1);
    if (rc != -EFAULT)
        linx_exit(62);

    rc = linx_syscall4(135, 0, 0, (long)old_mask, sizeof(old_mask));
    if (rc != 0)
        linx_exit(63);
    if (old_mask[0] != 0 || old_mask[1] != 0)
        linx_exit(64);

    if (linx_syscall3(226, (long)map, PAGE_SIZE, PROT_READ) != 0)
        linx_exit(65);
    if (linx_syscall3(64, 1, (long)map, 1) != 1)
        linx_exit(66);

    if (linx_syscall2(215, (long)map, PAGE_SIZE) != 0)
        linx_exit(67);

    if (linx_syscall3(64, 1, (long)ok, (long)(sizeof(ok) - 1)) != (long)(sizeof(ok) - 1))
        linx_exit(68);

    linx_exit(0);
}
