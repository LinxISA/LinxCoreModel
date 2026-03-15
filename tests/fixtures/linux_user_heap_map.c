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

#define PROT_READ 0x1
#define PROT_WRITE 0x2
#define MAP_PRIVATE 0x02
#define MAP_ANONYMOUS 0x20
#define PAGE_SIZE 4096L

__attribute__((noreturn)) void _start(void)
{
    static const char ok[] = "heap map ok\n";
    char *heap_base = (char *)linx_syscall1(214, 0);
    char *heap_top;
    char *map;
    long wrote;

    if ((long)heap_base <= 0)
        linx_exit(30);

    heap_top = (char *)linx_syscall1(214, (long)(heap_base + PAGE_SIZE));
    if (heap_top != heap_base + PAGE_SIZE)
        linx_exit(31);

    heap_base[0] = 'b';
    heap_base[1] = 'r';
    heap_base[2] = 'k';
    if (heap_base[0] != 'b' || heap_base[2] != 'k')
        linx_exit(32);

    if ((char *)linx_syscall1(214, (long)heap_base) != heap_base)
        linx_exit(33);

    map = (char *)linx_syscall6(
        222,
        0,
        PAGE_SIZE,
        PROT_READ | PROT_WRITE,
        MAP_PRIVATE | MAP_ANONYMOUS,
        -1,
        0);
    if ((long)map < 0)
        linx_exit(34);
    if (((long)map & (PAGE_SIZE - 1)) != 0)
        linx_exit(35);

    map[0] = 'm';
    map[1] = 'a';
    map[2] = 'p';
    map[3] = '\n';
    if (map[1] != 'a' || map[3] != '\n')
        linx_exit(36);

    if (linx_syscall2(215, (long)map, PAGE_SIZE) != 0)
        linx_exit(37);

    wrote = linx_syscall3(64, 1, (long)ok, (long)(sizeof(ok) - 1));
    if (wrote < 0)
        linx_exit(38);

    linx_exit(0);
}
