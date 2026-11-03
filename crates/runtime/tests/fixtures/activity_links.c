#include <assert.h>
#include <stdio.h>
#include <string.h>
#include "rinzu/native_diagnostics.h"

static void inspect(rinzu_activity_board_t *board,size_t expected) {
 size_t count=0;rinzu_activity_t **link=&board->head;
 for(rinzu_activity_t *record=board->head;record;record=record->next){
  assert(record->board==board && record->link==link && *link==record);
  link=&record->next;++count;
 }
 assert(count==expected);
}
static void clear_pending(rinzu_activity_board_t *board) {
 rinzu_activity_gate_lock(board);
 while(board->pending){
  rinzu_activity_t *record=board->pending;board->pending=record->pending_next;
  assert(!record->link && !record->next && board->stats.warning_pins);
  --board->stats.warning_pins;rinzu_activity_release_locked(record);
 }
 board->pending_tail=NULL;rinzu_activity_gate_unlock(board);
}
int main(int argc,char **argv) {
 rinzu_activity_board_t board={.owner=pthread_self()};
 rinzu_activity_gate_init(&board);
 rinzu_foreign_sync(pthread_cond_init(&board.changed,NULL));
 uint64_t owner=(uint64_t)(uintptr_t)board.owner;
 rinzu_activity_t *first=rinzu_activity_prepare(&board,RINZU_ACTIVITY_MANAGED,"first",1,2,3,owner,1);
 if(argc==2){
  if(!strcmp(argv[1],"unknown")){
   rinzu_activity_t fake={.board=&board,.creator=pthread_self(),.references=1};rinzu_activity_end(&fake);
  } else if(!strcmp(argv[1],"bad-link")){
   rinzu_activity_t *wrong=NULL;first->link=&wrong;rinzu_activity_end(first);
  } else if(!strcmp(argv[1],"double-end")){
   rinzu_activity_end(first);rinzu_activity_end(first);
  } else if(!strcmp(argv[1],"linked-release")){
   rinzu_activity_release_locked(first);
  } else assert(0);
  return 3;
 }
 rinzu_activity_end(first);inspect(&board,0);
 enum{COUNT=128};rinzu_activity_t *records[COUNT];unsigned char live[COUNT];
 for(size_t index=0;index<COUNT;++index){
  records[index]=rinzu_activity_prepare(&board,RINZU_ACTIVITY_MANAGED,"linked",11,31,49,owner,1);
  rinzu_activity_start(records[index]);live[index]=1;
 }
 inspect(&board,COUNT);
 /* An odd multiplier permutes128indices: head/middle/tail, allcache paths. */
 for(size_t step=0;step<COUNT;++step){
  size_t index=(step*37+63)%COUNT;rinzu_activity_end(records[index]);live[index]=0;
  inspect(&board,COUNT-step-1);rinzu_activity_t *cursor=board.head;
  for(size_t expected=COUNT;expected>0;--expected)if(live[expected-1]){
   assert(cursor==records[expected-1]);cursor=cursor->next;
  }
  assert(!cursor);
 }
 /* Late warning pin survives unlink and every subsequent cache reuse. */
 rinzu_activity_t *pinned=rinzu_activity_prepare(&board,RINZU_ACTIVITY_NATIVE,"pinned",17,21,34,owner,2);
 rinzu_activity_start(pinned);pinned->started=rinzu_activity_now()-UINT64_C(50000001);
 rinzu_activity_end(pinned);
 assert(board.pending==pinned && pinned->references==1 && !pinned->link && !pinned->next);
 char message[512];memcpy(message,pinned->message,sizeof message);size_t length=pinned->message_length;
 for(size_t reuse=0;reuse<128;++reuse){
  rinzu_activity_t *record=rinzu_activity_prepare(&board,RINZU_ACTIVITY_MANAGED,"reuse",1,2,3,owner,1);
  rinzu_activity_start(record);rinzu_activity_end(record);
  assert(record!=pinned && pinned->message_length==length && !memcmp(message,pinned->message,sizeof message));
 }
 clear_pending(&board);
 /* Fixed768Bclass stays fixed; the extra backlink reduces symbol fit by8B. */
 size_t fit=RINZU_ACTIVITY_CACHE_SLOT_BYTES-sizeof(rinzu_activity_t)-1;
 char symbol[RINZU_ACTIVITY_CACHE_SLOT_BYTES+1];memset(symbol,'x',sizeof symbol);symbol[fit]=0;
 rinzu_activity_t *short_record=rinzu_activity_prepare(&board,RINZU_ACTIVITY_MANAGED,symbol,0,0,0,owner,1);
 assert(short_record->bytes==RINZU_ACTIVITY_CACHE_SLOT_BYTES && strlen(short_record->symbol)==fit);rinzu_activity_end(short_record);
 symbol[fit]='x';symbol[fit+1]=0;
 rinzu_activity_t *long_record=rinzu_activity_prepare(&board,RINZU_ACTIVITY_MANAGED,symbol,0,0,0,owner,1);
 assert(long_record->bytes==sizeof(*long_record)+fit+2 && long_record->bytes>RINZU_ACTIVITY_CACHE_SLOT_BYTES);
 assert(!strcmp(long_record->symbol,symbol));rinzu_activity_end(long_record);
 inspect(&board,0);
 assert(board.stats.prepared==board.stats.ended && !board.stats.live && !board.stats.active && !board.stats.bytes && !board.stats.warning_pins);
 rinzu_activity_cache_drain(&board);assert(!board.stats.cached_bytes && !board.stats.cached_records);
 rinzu_foreign_sync(pthread_cond_destroy(&board.changed));rinzu_activity_gate_dispose(&board);
 puts("PASS activity backlink: permutation order membership empty/head/middle/tail pin reuse classbound stats0");
}
