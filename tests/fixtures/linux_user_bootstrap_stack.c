typedef unsigned long u64;

struct aux_pair {
    u64 key;
    u64 value;
};

extern void _start(void);
__attribute__((noreturn)) void bootstrap_main(u64 *initial_sp);

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

static inline __attribute__((noreturn)) void linx_exit(int code)
{
    (void)linx_syscall1(93, code);
    __builtin_unreachable();
}

static long string_eq(const char *lhs, const char *rhs)
{
    while (*lhs && *rhs) {
        if (*lhs != *rhs)
            return 0;
        lhs += 1;
        rhs += 1;
    }
    return *lhs == *rhs;
}

static long prefix_eq(const char *lhs, const char *prefix)
{
    while (*prefix) {
        if (*lhs != *prefix)
            return 0;
        lhs += 1;
        prefix += 1;
    }
    return 1;
}

static long env_contains(char **envp, const char *needle)
{
    long idx = 0;
    while (envp[idx]) {
        if (string_eq(envp[idx], needle))
            return 1;
        idx += 1;
    }
    return 0;
}

static const struct aux_pair *find_auxv(const struct aux_pair *auxv, u64 key)
{
    long idx = 0;
    while (auxv[idx].key != 0) {
        if (auxv[idx].key == key)
            return &auxv[idx];
        idx += 1;
    }
    return (const struct aux_pair *)0;
}

__attribute__((naked, noreturn)) void _start(void)
{
    __asm__ volatile(
        "c.movr sp, ->a0\n\t"
        "j bootstrap_main");
}

__attribute__((noreturn)) void bootstrap_main(u64 *sp)
{
    static const char ok[] = "bootstrap stack ok\n";
    char **argv = (char **)(sp + 1);
    char **envp;
    const struct aux_pair *auxv;
    const struct aux_pair *pagesz;
    const struct aux_pair *entry;
    const struct aux_pair *random;
    const struct aux_pair *execfn;
    long wrote;

    if (sp[0] != 3)
        linx_exit(40);
    if (!string_eq(argv[1], "alpha"))
        linx_exit(41);
    if (!string_eq(argv[2], "beta"))
        linx_exit(42);
    if (!prefix_eq(argv[0], "/Users/zhoubot/linx-isa/tools/LinxCoreModel/out/bringup/linux_user_bootstrap_stack.elf"))
        linx_exit(43);

    envp = argv + sp[0] + 1;
    while (*envp)
        envp += 1;
    envp += 1;
    auxv = (const struct aux_pair *)envp;

    if (!env_contains(argv + sp[0] + 1, "LX_BOOTSTRAP=1"))
        linx_exit(44);
    if (!env_contains(argv + sp[0] + 1, "LX_TRACE=1"))
        linx_exit(45);

    pagesz = find_auxv(auxv, 6);
    entry = find_auxv(auxv, 9);
    random = find_auxv(auxv, 25);
    execfn = find_auxv(auxv, 31);
    if (!pagesz || pagesz->value != 4096)
        linx_exit(46);
    if (!entry || entry->value != (u64)&_start)
        linx_exit(47);
    if (!random || !execfn)
        linx_exit(48);
    if (!string_eq((const char *)execfn->value, argv[0]))
        linx_exit(49);
    if (((const unsigned char *)random->value)[0] != 0xA5 ||
        ((const unsigned char *)random->value)[15] != 0xA5)
        linx_exit(50);

    wrote = linx_syscall3(64, 1, (long)ok, (long)(sizeof(ok) - 1));
    if (wrote < 0)
        linx_exit(51);
    linx_exit(0);
}
