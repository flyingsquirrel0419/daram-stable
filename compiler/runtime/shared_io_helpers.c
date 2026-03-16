DARAM_RUNTIME_LINKAGE void daram_print_str(const char* value) { fputs(value, stdout); }
DARAM_RUNTIME_LINKAGE void daram_println_str(const char* value) { fputs(value, stdout); fputc('\n', stdout); }
DARAM_RUNTIME_LINKAGE void daram_eprint_str(const char* value) { fputs(value, stderr); }
DARAM_RUNTIME_LINKAGE void daram_eprintln_str(const char* value) { fputs(value, stderr); fputc('\n', stderr); }
DARAM_RUNTIME_LINKAGE void daram_print_i64(long long value) { printf("%lld", value); }
DARAM_RUNTIME_LINKAGE void daram_println_i64(long long value) { printf("%lld\n", value); }
DARAM_RUNTIME_LINKAGE void daram_eprint_i64(long long value) { fprintf(stderr, "%lld", value); }
DARAM_RUNTIME_LINKAGE void daram_eprintln_i64(long long value) { fprintf(stderr, "%lld\n", value); }
