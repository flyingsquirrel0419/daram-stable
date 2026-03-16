typedef struct daram_unwind_frame {
  jmp_buf buf;
  struct daram_unwind_frame* prev;
  const char* message;
} daram_unwind_frame;

static daram_unwind_frame* daram_unwind_top = NULL;
static char daram_panic_buffer[256];

static void daram_enter_unwind(daram_unwind_frame* frame) {
  frame->prev = daram_unwind_top;
  frame->message = NULL;
  daram_unwind_top = frame;
}

static void daram_pop_unwind(void) {
  if (daram_unwind_top != NULL) {
    daram_unwind_top = daram_unwind_top->prev;
  }
}

static void daram_raise(const char* msg) {
  const char* message = msg ? msg : "panic";
  if (daram_unwind_top != NULL) {
    daram_unwind_top->message = message;
    longjmp(daram_unwind_top->buf, 1);
  }
  fprintf(stderr, "%s\n", message);
  exit(1);
}

DARAM_RUNTIME_LINKAGE void daram_resume_unwind_msg(const char* msg) {
  daram_raise(msg ? msg : "panic");
}

DARAM_RUNTIME_LINKAGE void daram_assert(bool cond) {
  if (!cond) {
    daram_raise("assertion failed");
  }
}

DARAM_RUNTIME_LINKAGE void daram_assert_eq_i64(long long lhs, long long rhs) {
  if (lhs != rhs) {
    snprintf(
        daram_panic_buffer,
        sizeof(daram_panic_buffer),
        "assert_eq failed: left=%lld right=%lld",
        lhs,
        rhs);
    daram_raise(daram_panic_buffer);
  }
}

DARAM_RUNTIME_LINKAGE void daram_panic_str(const char* msg) {
  daram_raise(msg);
}

DARAM_RUNTIME_LINKAGE void daram_panic_with_fmt_i64(
    const char* msg,
    long long lhs,
    long long rhs) {
  snprintf(
      daram_panic_buffer,
      sizeof(daram_panic_buffer),
      "%s: left=%lld right=%lld",
      msg,
      lhs,
      rhs);
  daram_raise(daram_panic_buffer);
}
