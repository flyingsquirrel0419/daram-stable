DARAM_RUNTIME_LINKAGE void daram_assert(unsigned char cond) {
    if (!cond) {
        fprintf(stderr, "assertion failed\n");
        exit(1);
    }
}

DARAM_RUNTIME_LINKAGE void daram_assert_eq_i64(long long lhs, long long rhs) {
    if (lhs != rhs) {
        fprintf(stderr, "assert_eq failed: left=%lld right=%lld\n", lhs, rhs);
        exit(1);
    }
}

DARAM_RUNTIME_LINKAGE void daram_panic_str(const char* msg) {
    fprintf(stderr, "%s\n", msg);
    exit(1);
}

DARAM_RUNTIME_LINKAGE void daram_panic_with_fmt_i64(
    const char* msg,
    long long lhs,
    long long rhs) {
    fprintf(stderr, "%s: left=%lld right=%lld\n", msg, lhs, rhs);
    exit(1);
}

typedef struct {
    uint64_t* data;
    size_t len;
    size_t cap;
} daram_vec_u64;

typedef struct {
    long long key;
    long long value;
} daram_hashmap_i64_entry;

typedef struct {
    daram_hashmap_i64_entry* entries;
    size_t len;
    size_t cap;
} daram_hashmap_i64;

typedef struct {
    char* key;
    long long value;
} daram_hashmap_str_i64_entry;

typedef struct {
    daram_hashmap_str_i64_entry* entries;
    size_t len;
    size_t cap;
} daram_hashmap_str_i64;

typedef struct {
    long long key;
    void* value;
} daram_hashmap_i64_ptr_entry;

typedef struct {
    daram_hashmap_i64_ptr_entry* entries;
    size_t len;
    size_t cap;
} daram_hashmap_i64_ptr;

typedef struct {
    char* key;
    void* value;
} daram_hashmap_str_ptr_entry;

typedef struct {
    daram_hashmap_str_ptr_entry* entries;
    size_t len;
    size_t cap;
} daram_hashmap_str_ptr;

static daram_vec_u64* daram_vec_alloc(void) {
    daram_vec_u64* vec = (daram_vec_u64*)calloc(1, sizeof(daram_vec_u64));
    if (!vec) {
        fprintf(stderr, "out of memory\n");
        exit(1);
    }
    return vec;
}

static void daram_vec_reserve(daram_vec_u64* vec, size_t needed) {
    if (vec->cap >= needed) {
        return;
    }
    size_t next = vec->cap ? vec->cap * 2 : 4;
    while (next < needed) {
        next *= 2;
    }
    uint64_t* data = (uint64_t*)realloc(vec->data, next * sizeof(uint64_t));
    if (!data) {
        fprintf(stderr, "out of memory\n");
        exit(1);
    }
    vec->data = data;
    vec->cap = next;
}

static daram_hashmap_i64* daram_hashmap_alloc(void) {
    daram_hashmap_i64* map = (daram_hashmap_i64*)calloc(1, sizeof(daram_hashmap_i64));
    if (!map) {
        fprintf(stderr, "out of memory\n");
        exit(1);
    }
    return map;
}

static daram_hashmap_str_i64* daram_hashmap_str_alloc(void) {
    daram_hashmap_str_i64* map =
        (daram_hashmap_str_i64*)calloc(1, sizeof(daram_hashmap_str_i64));
    if (!map) {
        fprintf(stderr, "out of memory\n");
        exit(1);
    }
    return map;
}

static daram_hashmap_i64_ptr* daram_hashmap_i64_ptr_alloc(void) {
    daram_hashmap_i64_ptr* map =
        (daram_hashmap_i64_ptr*)calloc(1, sizeof(daram_hashmap_i64_ptr));
    if (!map) {
        fprintf(stderr, "out of memory\n");
        exit(1);
    }
    return map;
}

static daram_hashmap_str_ptr* daram_hashmap_str_ptr_alloc(void) {
    daram_hashmap_str_ptr* map =
        (daram_hashmap_str_ptr*)calloc(1, sizeof(daram_hashmap_str_ptr));
    if (!map) {
        fprintf(stderr, "out of memory\n");
        exit(1);
    }
    return map;
}

static void daram_hashmap_reserve(daram_hashmap_i64* map, size_t needed) {
    if (map->cap >= needed) {
        return;
    }
    size_t next = map->cap ? map->cap * 2 : 4;
    while (next < needed) {
        next *= 2;
    }
    daram_hashmap_i64_entry* entries =
        (daram_hashmap_i64_entry*)realloc(map->entries, next * sizeof(daram_hashmap_i64_entry));
    if (!entries) {
        fprintf(stderr, "out of memory\n");
        exit(1);
    }
    map->entries = entries;
    map->cap = next;
}

static void daram_hashmap_str_reserve(daram_hashmap_str_i64* map, size_t needed) {
    if (map->cap >= needed) {
        return;
    }
    size_t next = map->cap ? map->cap * 2 : 4;
    while (next < needed) {
        next *= 2;
    }
    daram_hashmap_str_i64_entry* entries = (daram_hashmap_str_i64_entry*)realloc(
        map->entries, next * sizeof(daram_hashmap_str_i64_entry));
    if (!entries) {
        fprintf(stderr, "out of memory\n");
        exit(1);
    }
    map->entries = entries;
    map->cap = next;
}

static void daram_hashmap_i64_ptr_reserve(daram_hashmap_i64_ptr* map, size_t needed) {
    if (map->cap >= needed) {
        return;
    }
    size_t next = map->cap ? map->cap * 2 : 4;
    while (next < needed) {
        next *= 2;
    }
    daram_hashmap_i64_ptr_entry* entries = (daram_hashmap_i64_ptr_entry*)realloc(
        map->entries, next * sizeof(daram_hashmap_i64_ptr_entry));
    if (!entries) {
        fprintf(stderr, "out of memory\n");
        exit(1);
    }
    map->entries = entries;
    map->cap = next;
}

static void daram_hashmap_str_ptr_reserve(daram_hashmap_str_ptr* map, size_t needed) {
    if (map->cap >= needed) {
        return;
    }
    size_t next = map->cap ? map->cap * 2 : 4;
    while (next < needed) {
        next *= 2;
    }
    daram_hashmap_str_ptr_entry* entries = (daram_hashmap_str_ptr_entry*)realloc(
        map->entries, next * sizeof(daram_hashmap_str_ptr_entry));
    if (!entries) {
        fprintf(stderr, "out of memory\n");
        exit(1);
    }
    map->entries = entries;
    map->cap = next;
}

static long long daram_hashmap_find_i64(daram_hashmap_i64* map, long long key) {
    for (size_t i = 0; i < map->len; ++i) {
        if (map->entries[i].key == key) {
            return (long long)i;
        }
    }
    return -1;
}

static long long daram_hashmap_find_str(daram_hashmap_str_i64* map, const char* key) {
    for (size_t i = 0; i < map->len; ++i) {
        if (strcmp(map->entries[i].key, key) == 0) {
            return (long long)i;
        }
    }
    return -1;
}

static long long daram_hashmap_find_i64_ptr(daram_hashmap_i64_ptr* map, long long key) {
    for (size_t i = 0; i < map->len; ++i) {
        if (map->entries[i].key == key) {
            return (long long)i;
        }
    }
    return -1;
}

static long long daram_hashmap_find_str_ptr(daram_hashmap_str_ptr* map, const char* key) {
    for (size_t i = 0; i < map->len; ++i) {
        if (strcmp(map->entries[i].key, key) == 0) {
            return (long long)i;
        }
    }
    return -1;
}

static char* daram_strdup(const char* value) {
    size_t len = strlen(value);
    char* copy = (char*)malloc(len + 1);
    if (!copy) {
        fprintf(stderr, "out of memory\n");
        exit(1);
    }
    memcpy(copy, value, len + 1);
    return copy;
}

static void daram_write_option_i64(void* out, bool is_some, long long value) {
    uint64_t* words = (uint64_t*)out;
    words[0] = is_some ? 0 : 1;
    words[1] = (uint64_t)value;
}

static void daram_write_option_ptr(void* out, bool is_some, const void* value) {
    uint64_t* words = (uint64_t*)out;
    words[0] = is_some ? 0 : 1;
    words[1] = (uint64_t)(uintptr_t)value;
}

DARAM_RUNTIME_LINKAGE void* daram_vec_new(void) { return daram_vec_alloc(); }

DARAM_RUNTIME_LINKAGE void daram_vec_push_i64(void* raw, long long value) {
    daram_vec_u64* vec = (daram_vec_u64*)raw;
    daram_vec_reserve(vec, vec->len + 1);
    vec->data[vec->len++] = (uint64_t)value;
}

DARAM_RUNTIME_LINKAGE void daram_vec_push_ptr(void* raw, void* value) {
    daram_vec_u64* vec = (daram_vec_u64*)raw;
    daram_vec_reserve(vec, vec->len + 1);
    vec->data[vec->len++] = (uint64_t)(uintptr_t)value;
}

DARAM_RUNTIME_LINKAGE long long daram_vec_len(void* raw) {
    daram_vec_u64* vec = (daram_vec_u64*)raw;
    return (long long)vec->len;
}

DARAM_RUNTIME_LINKAGE void* daram_hashmap_new(void) { return daram_hashmap_alloc(); }

DARAM_RUNTIME_LINKAGE long long daram_hashmap_len(void* raw) {
    daram_hashmap_i64* map = (daram_hashmap_i64*)raw;
    return (long long)map->len;
}

DARAM_RUNTIME_LINKAGE void daram_hashmap_insert_i64_i64(
    void* raw,
    long long key,
    long long value,
    void* out) {
    daram_hashmap_i64* map = (daram_hashmap_i64*)raw;
    long long index = daram_hashmap_find_i64(map, key);
    if (index >= 0) {
        long long prev = map->entries[index].value;
        map->entries[index].value = value;
        daram_write_option_i64(out, true, prev);
        return;
    }
    daram_hashmap_reserve(map, map->len + 1);
    map->entries[map->len].key = key;
    map->entries[map->len].value = value;
    map->len += 1;
    daram_write_option_i64(out, false, 0);
}

DARAM_RUNTIME_LINKAGE void daram_hashmap_get_i64_ref_i64(void* raw, long long key, void* out) {
    daram_hashmap_i64* map = (daram_hashmap_i64*)raw;
    long long index = daram_hashmap_find_i64(map, key);
    if (index >= 0) {
        daram_write_option_ptr(out, true, &map->entries[index].value);
    } else {
        daram_write_option_ptr(out, false, NULL);
    }
}

DARAM_RUNTIME_LINKAGE void daram_hashmap_remove_i64_i64(void* raw, long long key, void* out) {
    daram_hashmap_i64* map = (daram_hashmap_i64*)raw;
    long long index = daram_hashmap_find_i64(map, key);
    if (index < 0) {
        daram_write_option_i64(out, false, 0);
        return;
    }
    long long prev = map->entries[index].value;
    size_t last = map->len - 1;
    if ((size_t)index != last) {
        map->entries[index] = map->entries[last];
    }
    map->len -= 1;
    daram_write_option_i64(out, true, prev);
}

DARAM_RUNTIME_LINKAGE void daram_hashmap_insert_str_i64(
    void* raw,
    const char* key,
    long long value,
    void* out) {
    daram_hashmap_str_i64* map = (daram_hashmap_str_i64*)raw;
    long long index = daram_hashmap_find_str(map, key);
    if (index >= 0) {
        long long prev = map->entries[index].value;
        map->entries[index].value = value;
        daram_write_option_i64(out, true, prev);
        return;
    }
    daram_hashmap_str_reserve(map, map->len + 1);
    map->entries[map->len].key = daram_strdup(key);
    map->entries[map->len].value = value;
    map->len += 1;
    daram_write_option_i64(out, false, 0);
}

DARAM_RUNTIME_LINKAGE void daram_hashmap_get_str_ref_i64(void* raw, const char* key, void* out) {
    daram_hashmap_str_i64* map = (daram_hashmap_str_i64*)raw;
    long long index = daram_hashmap_find_str(map, key);
    if (index >= 0) {
        daram_write_option_ptr(out, true, &map->entries[index].value);
    } else {
        daram_write_option_ptr(out, false, NULL);
    }
}

DARAM_RUNTIME_LINKAGE void daram_hashmap_remove_str_i64(void* raw, const char* key, void* out) {
    daram_hashmap_str_i64* map = (daram_hashmap_str_i64*)raw;
    long long index = daram_hashmap_find_str(map, key);
    if (index < 0) {
        daram_write_option_i64(out, false, 0);
        return;
    }
    long long prev = map->entries[index].value;
    free(map->entries[index].key);
    size_t last = map->len - 1;
    if ((size_t)index != last) {
        map->entries[index] = map->entries[last];
    }
    map->len -= 1;
    daram_write_option_i64(out, true, prev);
}

DARAM_RUNTIME_LINKAGE void daram_hashmap_insert_i64_ptr(
    void* raw,
    long long key,
    void* value,
    void* out) {
    daram_hashmap_i64_ptr* map = (daram_hashmap_i64_ptr*)raw;
    long long index = daram_hashmap_find_i64_ptr(map, key);
    if (index >= 0) {
        void* prev = map->entries[index].value;
        map->entries[index].value = value;
        daram_write_option_ptr(out, true, prev);
        return;
    }
    daram_hashmap_i64_ptr_reserve(map, map->len + 1);
    map->entries[map->len].key = key;
    map->entries[map->len].value = value;
    map->len += 1;
    daram_write_option_ptr(out, false, NULL);
}

DARAM_RUNTIME_LINKAGE void daram_hashmap_get_i64_ref_ptr(void* raw, long long key, void* out) {
    daram_hashmap_i64_ptr* map = (daram_hashmap_i64_ptr*)raw;
    long long index = daram_hashmap_find_i64_ptr(map, key);
    if (index >= 0) {
        daram_write_option_ptr(out, true, &map->entries[index].value);
    } else {
        daram_write_option_ptr(out, false, NULL);
    }
}

DARAM_RUNTIME_LINKAGE void daram_hashmap_remove_i64_ptr(void* raw, long long key, void* out) {
    daram_hashmap_i64_ptr* map = (daram_hashmap_i64_ptr*)raw;
    long long index = daram_hashmap_find_i64_ptr(map, key);
    if (index < 0) {
        daram_write_option_ptr(out, false, NULL);
        return;
    }
    void* prev = map->entries[index].value;
    size_t last = map->len - 1;
    if ((size_t)index != last) {
        map->entries[index] = map->entries[last];
    }
    map->len -= 1;
    daram_write_option_ptr(out, true, prev);
}

DARAM_RUNTIME_LINKAGE void daram_hashmap_insert_str_ptr(
    void* raw,
    const char* key,
    void* value,
    void* out) {
    daram_hashmap_str_ptr* map = (daram_hashmap_str_ptr*)raw;
    long long index = daram_hashmap_find_str_ptr(map, key);
    if (index >= 0) {
        void* prev = map->entries[index].value;
        map->entries[index].value = value;
        daram_write_option_ptr(out, true, prev);
        return;
    }
    daram_hashmap_str_ptr_reserve(map, map->len + 1);
    map->entries[map->len].key = daram_strdup(key);
    map->entries[map->len].value = value;
    map->len += 1;
    daram_write_option_ptr(out, false, NULL);
}

DARAM_RUNTIME_LINKAGE void daram_hashmap_get_str_ref_ptr(void* raw, const char* key, void* out) {
    daram_hashmap_str_ptr* map = (daram_hashmap_str_ptr*)raw;
    long long index = daram_hashmap_find_str_ptr(map, key);
    if (index >= 0) {
        daram_write_option_ptr(out, true, &map->entries[index].value);
    } else {
        daram_write_option_ptr(out, false, NULL);
    }
}

DARAM_RUNTIME_LINKAGE void daram_hashmap_remove_str_ptr(void* raw, const char* key, void* out) {
    daram_hashmap_str_ptr* map = (daram_hashmap_str_ptr*)raw;
    long long index = daram_hashmap_find_str_ptr(map, key);
    if (index < 0) {
        daram_write_option_ptr(out, false, NULL);
        return;
    }
    void* prev = map->entries[index].value;
    free(map->entries[index].key);
    size_t last = map->len - 1;
    if ((size_t)index != last) {
        map->entries[index] = map->entries[last];
    }
    map->len -= 1;
    daram_write_option_ptr(out, true, prev);
}
