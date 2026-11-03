/* Reads only the child PID supplied by the owning benchmark harness. */
#include <libproc.h>
#include <mach/mach_time.h>
#include <stdio.h>
#include <stdlib.h>
#include <sys/proc_info.h>
int main(int argc, char **argv) {
  if (argc != 2)
    return 2;
  int pid = atoi(argv[1]);
  if (pid <= 0)
    return 2;
  struct proc_taskinfo task;
  if (proc_pidinfo(pid, PROC_PIDTASKINFO, 0, &task, sizeof(task)) != sizeof(task))
    return 3;
  mach_timebase_info_data_t timebase;
  if (mach_timebase_info(&timebase) != KERN_SUCCESS || !timebase.denom)
    return 4;
  unsigned long long user =
      (unsigned long long)((__uint128_t)task.pti_total_user * timebase.numer / timebase.denom);
  unsigned long long system =
      (unsigned long long)((__uint128_t)task.pti_total_system * timebase.numer / timebase.denom);
  printf("{\"pid\":%d,\"rss\":%llu,\"virtual\":%llu,\"userNs\":%llu,\"systemNs\":%llu,\"threads\":%"
         "d,\"switches\":%d,\"sockets\":[",
         pid, task.pti_resident_size, task.pti_virtual_size, user, system, task.pti_threadnum,
         task.pti_csw);
  struct proc_fdinfo descriptors[4096];
  int size = proc_pidinfo(pid, PROC_PIDLISTFDS, 0, descriptors, sizeof(descriptors));
  int first = 1;
  for (int i = 0; i < size / (int)sizeof(descriptors[0]); ++i) {
    if (descriptors[i].proc_fdtype != PROX_FDTYPE_SOCKET)
      continue;
    struct socket_fdinfo socket;
    if (proc_pidfdinfo(pid, descriptors[i].proc_fd, PROC_PIDFDSOCKETINFO, &socket,
                       sizeof(socket)) != sizeof(socket))
      continue;
    if (socket.psi.soi_kind != SOCKINFO_TCP)
      continue;
    printf("%s{\"fd\":%d,\"sendHighWater\":%u,\"receiveHighWater\":%u,\"sendQueued\":%u,"
           "\"receiveQueued\":%u}",
           first ? "" : ",", descriptors[i].proc_fd, socket.psi.soi_snd.sbi_hiwat,
           socket.psi.soi_rcv.sbi_hiwat, socket.psi.soi_snd.sbi_cc, socket.psi.soi_rcv.sbi_cc);
    first = 0;
  }
  puts("]}");
}
