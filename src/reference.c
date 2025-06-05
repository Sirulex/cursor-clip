#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <stdbool.h>
#include <time.h>
#include <assert.h>
#include <errno.h>
#include <fcntl.h>
#include <sys/mman.h>
#include <unistd.h>

#include <wayland-client-protocol.h>
#include <wayland-client.h>

#include "single-pixel-buffer-v1.h"
#include "wlr-layer-shell-unstable-v1.h"
#include "wlr-virtual-pointer-unstable-v1.h"
#include "viewporter.h"

// for arguments
#include "arg.h"
char *argv0;

char *emulate_cmd = NULL;

// default color in 0xAARRGGBB format.
uint32_t color = 0xcfd79921;

// default animation duration, 1 second.
int animation_duration_in_second = 1;

// if user defined square size
int size = 0;

// control flag to show animation or not.
bool no_animation = false;

// false to exit
bool running = true;

// state
struct wl_display *display = NULL;
struct wl_registry *registry = NULL;
struct wl_compositor *compositor = NULL;
struct wl_surface *surface = NULL;
struct zwlr_layer_shell_v1 *layer_shell = NULL;
struct wp_viewporter *viewporter = NULL;
struct wp_single_pixel_buffer_manager_v1 *single_pixel_buffer_manager = NULL;
struct zwlr_virtual_pointer_manager_v1 *virtual_pointer_manager = NULL;

// output
// using NULL for focused output
struct wl_output *output = NULL;
struct zwlr_layer_surface_v1 *layer_surface = NULL;
struct wp_viewport *viewport = NULL;
struct wl_callback *frame_callback = NULL;
int surface_width = 0;
int surface_height = 0;

// seat
struct wl_seat *seat = NULL;
struct wl_pointer *pointer = NULL;
struct zwlr_virtual_pointer_v1 *virtual_pointer = NULL;
// set it in pointer_handle_enter
int cursor_x;
int cursor_y;


// shm
struct wl_shm *shm = NULL;
struct wl_buffer *shm_buffer = NULL;


int64_t start_ms = 0;
int64_t delay_ms = 0;

static int64_t now_ms(void) {
  struct timespec ts = {0};
  if (clock_gettime(CLOCK_MONOTONIC, &ts) != 0) {
    perror("clock_gettime() failed");
    exit(1);
  }
  return (int64_t)ts.tv_sec * 1000 + ts.tv_nsec / 1000000;
}

static void randname(char *buf)
{
  struct timespec ts;
  clock_gettime(CLOCK_REALTIME, &ts);
  long r = ts.tv_nsec;
  for (int i = 0; i < 6; ++i) {
    buf[i] = 'A'+(r&15)+(r&16)*2;
    r >>= 5;
  }
}

static int create_shm_file(void)
{
  int retries = 100;
  do {
    char name[] = "/wl_shm-XXXXXX";
    randname(name + sizeof(name) - 7);
    --retries;
    int fd = shm_open(name, O_RDWR | O_CREAT | O_EXCL, 0600);
    if (fd >= 0) {
      shm_unlink(name);
      return fd;
    }
  } while (retries > 0 && errno == EEXIST);
  return -1;
}

int allocate_shm_file(size_t size)
{
  int fd = create_shm_file();
  if (fd < 0)
    return -1;
  int ret;
  do {
    ret = ftruncate(fd, size);
  } while (ret < 0 && errno == EINTR);
  if (ret < 0) {
    close(fd);
    return -1;
  }
  return fd;
}


static void update_pixels(uint32_t *pixels);

static void frame_callback_handle_done(void *data, struct wl_callback *callback, uint32_t time) {
  assert(callback == frame_callback);
  wl_callback_destroy(callback);
  frame_callback = NULL;
  update_pixels((uint32_t *)data);
}

static const struct wl_callback_listener frame_callback_listener = {
  .done = frame_callback_handle_done,
};

static void update_pixels(uint32_t *pixels) {
  int64_t delta = now_ms() - start_ms;
  double progress = (double)delta / delay_ms;
  if (progress >= 1) {
    running = false;
  }
  //uint32_t alpha = progress * UINT32_MAX ;

  //don't use uint32_t here. since 'cursor_x - half_size' can be negative.
  int half_size = (surface_height < surface_width ? surface_height : surface_width)/10;

  //user defined square size
  if(size != 0)
      half_size = size >> 1;

  half_size = half_size * progress;

  // since these not changed at runtime,
  // it's not neccesary put them in loop.
  //uint32_t color = alpha << 24 | red << 16 | green << 8 | blue;

  for (int y = 0; y < surface_height; y++) {
    if(y < cursor_y - half_size || y > cursor_y + half_size)
      continue;
    for (int x = 0; x < surface_width; x++) {
      if(x < cursor_x - half_size || x > cursor_x + half_size)
        continue;
      pixels[x + (y * surface_width)] = color;
    }
  }

  wl_surface_attach(surface, shm_buffer, 0, 0);
  wp_viewport_set_destination(viewport, surface_width, surface_height);

  frame_callback = wl_surface_frame(surface);
  wl_callback_add_listener(frame_callback, &frame_callback_listener, pixels);
  wl_surface_damage(surface, 0, 0, INT32_MAX, INT32_MAX);
  wl_surface_commit(surface);
}

static void pointer_handle_enter(void *data, struct wl_pointer *wl_pointer, uint32_t serial, struct wl_surface *surface, wl_fixed_t surface_x, wl_fixed_t surface_y) {

  cursor_x = wl_fixed_to_int(surface_x);
  cursor_y = wl_fixed_to_int(surface_y);

  printf("%d %d\n", cursor_x, cursor_y);

  if(no_animation == true) {
    running = false;
    return;
  }

  const int width = surface_width, height = surface_height;
  const int stride = width * 4;
  const int shm_pool_size = height * stride * 2;

  int fd = allocate_shm_file(shm_pool_size);
  uint8_t *pool_data = mmap(NULL, shm_pool_size,
      PROT_READ | PROT_WRITE, MAP_SHARED, fd, 0);
  struct wl_shm_pool *pool = wl_shm_create_pool(shm, fd, shm_pool_size);
  int index = 0;
  int offset = height * stride * index;

  shm_buffer = wl_shm_pool_create_buffer(pool, offset, width, height, stride, WL_SHM_FORMAT_ARGB8888);

  wl_shm_pool_destroy(pool);

  uint32_t *pixels = (uint32_t *)&pool_data[offset];

  update_pixels(pixels);
}

static void pointer_handle_leave(void *data, struct wl_pointer *wl_pointer,
    uint32_t serial, struct wl_surface *surface) {
  running = false;
}

static void pointer_handle_motion(void *data, struct wl_pointer *wl_pointer,
    uint32_t time, wl_fixed_t surface_x, wl_fixed_t surface_y) {
  running = false;
}

static void pointer_handle_button(void *data, struct wl_pointer *wl_pointer,
    uint32_t serial, uint32_t time, uint32_t button, uint32_t state) {
  running = false;
}

static void pointer_handle_axis(void *data, struct wl_pointer *wl_pointer,
    uint32_t time, uint32_t axis, wl_fixed_t value) {
  running = false;
}

static const struct wl_pointer_listener pointer_listener = {
  .enter = pointer_handle_enter,
  .leave = pointer_handle_leave,
  .motion = pointer_handle_motion,
  .button = pointer_handle_button,
  .axis = pointer_handle_axis,
};

static void seat_handle_capabilities(void *data, struct wl_seat *wl_seat, uint32_t caps) {

  if (caps & WL_SEAT_CAPABILITY_POINTER) {
    pointer = wl_seat_get_pointer(seat);
    wl_pointer_add_listener(pointer, &pointer_listener, NULL);
  }

  if(virtual_pointer_manager != NULL) {
    virtual_pointer = zwlr_virtual_pointer_manager_v1_create_virtual_pointer(virtual_pointer_manager, seat);
  }
}

static const struct wl_seat_listener seat_listener = {
  .capabilities = seat_handle_capabilities,
};

static void global_registry_handler(void *data, struct wl_registry *registry,
    uint32_t id, const char *interface, uint32_t version)
{
  if (strcmp(interface, wl_shm_interface.name) == 0) {
    shm = wl_registry_bind(registry, id, &wl_shm_interface, 1);
  } else if (strcmp(interface, wl_compositor_interface.name) == 0) {
    compositor = wl_registry_bind(registry, id, &wl_compositor_interface, 4);
  } else if (strcmp(interface, zwlr_layer_shell_v1_interface.name) == 0) {
    layer_shell = wl_registry_bind(registry, id, &zwlr_layer_shell_v1_interface, 2);
  } else if (strcmp(interface, wp_viewporter_interface.name) == 0) {
    viewporter = wl_registry_bind(registry, id, &wp_viewporter_interface, 1);
  } else if (strcmp(interface, wp_single_pixel_buffer_manager_v1_interface.name) == 0) {
    single_pixel_buffer_manager = wl_registry_bind(registry, id, &wp_single_pixel_buffer_manager_v1_interface, 1);
  } else if (strcmp(interface, zwlr_virtual_pointer_manager_v1_interface.name) == 0) {
    virtual_pointer_manager = wl_registry_bind(registry, id, &zwlr_virtual_pointer_manager_v1_interface, 2);
  }
  else if (strcmp(interface, wl_seat_interface.name) == 0) {
    seat = wl_registry_bind(registry, id, &wl_seat_interface, 1);
    wl_seat_add_listener(seat, &seat_listener, NULL);//FIXME.
  }

  // if output is NULL, it will default to focused output, that's what we want.
  /*else if (strcmp(interface, wl_output_interface.name) == 0) {
      wl_registry_bind(registry, id, &wl_output_interface, 4);
      wl_output_add_listener(output, &wl_output_listener, output);
  } */
}

static void global_registry_remove_handler(void *data, struct wl_registry *registry, uint32_t id)
{
  wl_pointer_destroy(pointer);
  wl_seat_destroy(seat);
}

static const struct wl_registry_listener registry_listener = {
  .global = global_registry_handler,
  .global_remove = global_registry_remove_handler,
};

static void layer_surface_handle_configure(void *data, struct zwlr_layer_surface_v1 *layer_surface, uint32_t serial, uint32_t width, uint32_t height) {
  surface_width = width;
  surface_height = height;

  zwlr_layer_surface_v1_ack_configure(layer_surface, serial);

  struct wl_buffer *buffer = wp_single_pixel_buffer_manager_v1_create_u32_rgba_buffer(single_pixel_buffer_manager, 0, 0, 0, 0);
  wl_surface_attach(surface, buffer, 0, 0);

  wp_viewport_set_destination(viewport, surface_width, surface_height);
  wl_surface_damage(surface, 0, 0, INT32_MAX, INT32_MAX);
  wl_surface_commit(surface);

  wl_buffer_destroy(buffer);

  //move cursor to activate pointer enter handler.
  if(virtual_pointer_manager != NULL) {
    zwlr_virtual_pointer_v1_motion(virtual_pointer, 0, 0, 0);
    zwlr_virtual_pointer_v1_frame(virtual_pointer);
  } else {
    // return value is unused, using 'system' is enough.
    system(emulate_cmd);
  }
}

static void layer_surface_handle_closed(void *data, struct zwlr_layer_surface_v1 *layer_surface) {
  if (frame_callback != NULL) {
    wl_callback_destroy(frame_callback);
  }
  wp_viewport_destroy(viewport);
  zwlr_layer_surface_v1_destroy(layer_surface);
  wl_surface_destroy(surface);
}

static const struct zwlr_layer_surface_v1_listener layer_surface_listener = {
  .configure = layer_surface_handle_configure,
  .closed = layer_surface_handle_closed,
};

void usage() {
  printf("wl-find-cursor - highlight and report cursor position in wayland.\n\n");
  printf("Options:\n");
  printf("  -d <int>    : animation duration in second.\n");
  printf("  -s <int>    : animation square size.\n");
  printf("  -c <hex int>: animation square color in 0xAARRGGBB format.\n");
  printf("  -p          : skip animation.\n");
  printf("  -e <string> : command to emulate mouse move event.\n");
  printf("  -h          : show this message.\n");
  exit(0);
}
int main(int argc, char *argv[])
{
  ARGBEGIN {
  case 'c':
    color = strtol(EARGF(usage()), NULL, 0);
    break;
  case 'd':
    animation_duration_in_second = strtol(EARGF(usage()), NULL, 0);
    break;
  case 's':
    size = strtol(EARGF(usage()), NULL, 0);
    break;
  case 'p':
    no_animation = true;
    break;
  case 'e':
    emulate_cmd = EARGF(usage());
    break;
  case 'h':
    usage();
    exit(0);
  default:
    usage();
  }
  ARGEND;

  display = wl_display_connect(NULL);
  if (display == NULL) {
      exit(1);
  }

  registry = wl_display_get_registry(display);
  wl_registry_add_listener(registry, &registry_listener, NULL);


  wl_display_roundtrip(display);

  struct {
    const char *name;
    bool found;
  } required_globals[] = {
    { "wl_compositor", compositor != NULL },
    { "wl_shm", shm != NULL },
    { "zwlr_layer_shell_v1", layer_shell != NULL },
    { "wp_viewporter", viewporter != NULL },
    { "wp_single_pixel_buffer_manager_v1", single_pixel_buffer_manager != NULL },
    { "zwlr_virtual_pointer_manager_v1", virtual_pointer_manager != NULL },
  };
  for (size_t i = 0; i < sizeof(required_globals) / sizeof(required_globals[0]); i++) {
    if (!required_globals[i].found) {
      if(!strcmp("zwlr_virtual_pointer_manager_v1", required_globals[i].name)) {
        if (emulate_cmd != NULL) {
          break;
        } else {
          fprintf(stderr, "The compositor lack of zwlr_virtual_pointer_manager_v1 support!\n");
          fprintf(stderr, "Please specify a cmd by 'wl-find-cursor -c' to emulate mouse event.\n\n");
          fprintf(stderr, "For example:\n");
          fprintf(stderr, "wl-find-cursor -c \"swaymsg 'seat - cursor move 0 0'\"\n");
          fprintf(stderr, "wl-find-cursor -c \"ydotool mousemove 0 1\"\n");
          return 1;
        }
      } else {
        fprintf(stderr, "The compositor lack %s support!\n", required_globals[i].name);
        return 1;
      }
    }
  }

  wl_display_dispatch(display);

  surface = wl_compositor_create_surface(compositor);
  if (surface == NULL) {
      exit(1);
  }

  layer_surface = zwlr_layer_shell_v1_get_layer_surface(layer_shell, surface, output, ZWLR_LAYER_SHELL_V1_LAYER_OVERLAY, "find-cursor");
  zwlr_layer_surface_v1_add_listener(layer_surface, &layer_surface_listener, NULL); //FIXME.

  viewport = wp_viewporter_get_viewport(viewporter, surface);
  zwlr_layer_surface_v1_set_anchor(layer_surface,
    ZWLR_LAYER_SURFACE_V1_ANCHOR_TOP |
    ZWLR_LAYER_SURFACE_V1_ANCHOR_LEFT |
    ZWLR_LAYER_SURFACE_V1_ANCHOR_RIGHT |
    ZWLR_LAYER_SURFACE_V1_ANCHOR_BOTTOM);
  zwlr_layer_surface_v1_set_keyboard_interactivity(layer_surface, false);
  zwlr_layer_surface_v1_set_exclusive_zone(layer_surface, -1);
  wl_surface_commit(surface);

  delay_ms = animation_duration_in_second * 1000;
  start_ms = now_ms();

  // Display loop.
  while (running) {
    if (wl_display_dispatch(display) < 0) {
      break;
    }
  }

  if (frame_callback != NULL) {
      wl_callback_destroy(frame_callback);
  }
  wp_viewport_destroy(viewport);
  zwlr_layer_surface_v1_destroy(layer_surface);
  wl_surface_destroy(surface);

  wl_pointer_destroy(pointer);
  wl_seat_destroy(seat);

  wl_compositor_destroy(compositor);
  zwlr_layer_shell_v1_destroy(layer_shell);
  wp_viewporter_destroy(viewporter);
  wp_single_pixel_buffer_manager_v1_destroy(single_pixel_buffer_manager);
  wl_registry_destroy(registry);
  wl_display_disconnect(display);

  return 0;
}
