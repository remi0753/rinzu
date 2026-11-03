CC ?= cc
CFLAGS := -std=c11 -O1 -g -Wall -Wextra -Werror -pthread
BUILD_DIR := build
SOURCE := callback_fixture.c
HOST_SOURCE := host_session_fixture.c
ROOT := ../../..
RUNTIME_LIB_DIR := $(abspath $(ROOT)/target/debug)
RUNTIME_LINK := -L$(RUNTIME_LIB_DIR) -lrinzu_runtime -Wl,-rpath,$(RUNTIME_LIB_DIR)
HOST_CFLAGS := $(CFLAGS) -I$(ROOT)/include

.PHONY: sanitize memory-sanitize thread-sanitize race-sanitize production-sanitize production-thread-sanitize runtime clean

sanitize: $(BUILD_DIR)/callback_fixture_asan production-sanitize
	./$(BUILD_DIR)/callback_fixture_asan

thread-sanitize: $(BUILD_DIR)/callback_fixture_tsan production-thread-sanitize
	./$(BUILD_DIR)/callback_fixture_tsan

memory-sanitize: sanitize

race-sanitize: thread-sanitize

production-sanitize: $(BUILD_DIR)/host_session_fixture_asan
	./$(BUILD_DIR)/host_session_fixture_asan

production-thread-sanitize: $(BUILD_DIR)/host_session_fixture_tsan
	./$(BUILD_DIR)/host_session_fixture_tsan

runtime:
	cargo build --manifest-path $(ROOT)/Cargo.toml -p rinzu-runtime

$(BUILD_DIR):
	mkdir -p $(BUILD_DIR)

$(BUILD_DIR)/callback_fixture_asan: $(SOURCE) | $(BUILD_DIR)
	$(CC) $(CFLAGS) -fsanitize=address,undefined -fno-omit-frame-pointer $< -o $@

$(BUILD_DIR)/callback_fixture_tsan: $(SOURCE) | $(BUILD_DIR)
	$(CC) $(CFLAGS) -fsanitize=thread -fno-omit-frame-pointer $< -o $@

$(BUILD_DIR)/host_session_fixture_asan: $(HOST_SOURCE) runtime | $(BUILD_DIR)
	$(CC) $(HOST_CFLAGS) -fsanitize=address,undefined -fno-omit-frame-pointer $< $(RUNTIME_LINK) -o $@

$(BUILD_DIR)/host_session_fixture_tsan: $(HOST_SOURCE) runtime | $(BUILD_DIR)
	$(CC) $(HOST_CFLAGS) -fsanitize=thread -fno-omit-frame-pointer $< $(RUNTIME_LINK) -o $@

clean:
	rm -rf $(BUILD_DIR)
