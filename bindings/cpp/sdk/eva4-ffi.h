#ifndef EVA4_FFI_H
#define EVA4_FFI_H

const int16_t EVA_FFI_SVC_OP_IS_ACTIVE = 1;

const int16_t EVA_FFI_SVC_OP_SUBSCRIBE_TOPIC = 10;
const int16_t EVA_FFI_SVC_OP_UNSUBSCRIBE_TOPIC = 11;
const int16_t EVA_FFI_SVC_OP_PUBLISH_TOPIC = 12;
const int16_t EVA_FFI_SVC_OP_RPC_CALL = 20;
const int16_t EVA_FFI_SVC_OP_GET_RPC_RESULT = 29;

const int16_t EVA_FFI_SVC_OP_LOG_TRACE = 100;
const int16_t EVA_FFI_SVC_OP_LOG_DEBUG = 110;
const int16_t EVA_FFI_SVC_OP_LOG_INFO = 120;
const int16_t EVA_FFI_SVC_OP_LOG_WARN = 130;
const int16_t EVA_FFI_SVC_OP_LOG_ERROR = 140;

const int16_t EVA_FFI_SVC_OP_TERMINATE = -99;
const int16_t EVA_FFI_SVC_OP_POC = -100;

struct EvaFFIBuffer {
  size_t len;
  char* data;
  size_t max;
};

struct EvaFFIFrame {
  unsigned char kind;
  char *primary_sender;
  char *topic;
  size_t payload_size;
  char *payload;
};

struct EvaFFIRpcEvent {
  char *primary_sender;
  size_t method_size;
  char *method; // the method is allowed to be binary
  size_t payload_size;
  char *payload;
};

#endif
