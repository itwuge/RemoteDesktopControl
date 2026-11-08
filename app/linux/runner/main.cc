// 用途：Linux runner 进程入口（创建 MyApplication 并进入 GTK 主循环）。
#include "my_application.h"

int main(int argc, char** argv) {
  g_autoptr(MyApplication) app = my_application_new();
  return g_application_run(G_APPLICATION(app), argc, argv);
}
