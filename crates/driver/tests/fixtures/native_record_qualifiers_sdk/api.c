#include "api.h"
#include <stdlib.h>
Read qualifier_aliases(Read read,Write write,Handler handler) {
    if(read!=write) abort();
    write->x+=7;
    if(handler(write,read)!=-11) abort();
    write->x+=5;
    return read;
}
int64_t qualifier_change(Write point,int64_t amount) {
    point->x+=amount;
    return 0;
}
