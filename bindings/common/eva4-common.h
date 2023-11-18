#ifndef EVA4_COMMON_H
#define EVA4_COMMON_H

const int16_t EVA_ITEM_STATUS_OK = 1;
const int16_t EVA_ITEM_STATUS_ERROR = -1;

const int16_t EVA_OK = 0;
const int16_t EVA_ERR_CODE_NOT_FOUND = -32001;
const int16_t EVA_ERR_CODE_ACCESS_DENIED = -32002;
const int16_t EVA_ERR_CODE_SYSTEM_ERROR = -32003;
const int16_t EVA_ERR_CODE_OTHER = -32004;
const int16_t EVA_ERR_CODE_NOT_READY = -32005;
const int16_t EVA_ERR_CODE_UNSUPPORTED = -32006;
const int16_t EVA_ERR_CODE_CORE_ERROR = -32007;
const int16_t EVA_ERR_CODE_TIMEOUT = -32008;
const int16_t EVA_ERR_CODE_INVALID_DATA = -32009;
const int16_t EVA_ERR_CODE_FUNC_FAILED = -32010;
const int16_t EVA_ERR_CODE_ABORTED = -32011;
const int16_t EVA_ERR_CODE_ALREADY_EXISTS = -32012;
const int16_t EVA_ERR_CODE_BUSY = -32013;
const int16_t EVA_ERR_CODE_METHOD_NOT_IMPLEMENTED = -32014;
const int16_t EVA_ERR_CODE_TOKEN_RESTRICTED = -32015;
const int16_t EVA_ERR_CODE_IO = -32016;
const int16_t EVA_ERR_CODE_REGISTRY = -32017;
const int16_t EVA_ERR_CODE_EVAHI_AUTH_REQUIRED = -32018;
const int16_t EVA_ERR_CODE_ACCESS_DENIED_MORE_DATA_REQUIRED = -32022;
const int16_t EVA_ERR_CODE_PARSE = -32700;
const int16_t EVA_ERR_CODE_INVALID_REQUEST = -32600;
const int16_t EVA_ERR_CODE_METHOD_NOT_FOUND = -32601;
const int16_t EVA_ERR_CODE_INVALID_PARAMS = -32602;
const int16_t EVA_ERR_CODE_INTERNAL_RPC = -32603;
const int16_t EVA_ERR_CODE_BUS_CLIENT_NOT_REGISTERED = -32113;
const int16_t EVA_ERR_CODE_BUS_DATA = -32114;
const int16_t EVA_ERR_CODE_BUS_IO = -32115;
const int16_t EVA_ERR_CODE_BUS_OTHER = -32116;
const int16_t EVA_ERR_CODE_BUS_NOT_SUPPORTED = -32117;
const int16_t EVA_ERR_CODE_BUS_BUSY = -32118;
const int16_t EVA_ERR_CODE_BUS_NOT_DELIVERED = -32119;
const int16_t EVA_ERR_CODE_BUS_TIMEOUT = -32120;
const int16_t EVA_ERR_CODE_BUS_ACCESS = -32121;

const uint8_t EVA_ACTION_STATUS_CREATED = 0b00000000;
const uint8_t EVA_ACTION_STATUS_ACCEPTED = 0b00000001;
const uint8_t EVA_ACTION_STATUS_PENDING = 0b00000010;
const uint8_t EVA_ACTION_STATUS_RUNNING = 0b00001000;
const uint8_t EVA_ACTION_STATUS_COMPLETED = 0b00001111;
const uint8_t EVA_ACTION_STATUS_FAILED = 0b10000000;
const uint8_t EVA_ACTION_STATUS_CANCELED = 0b10000001;
const uint8_t EVA_ACTION_STATUS_TERMINATED = 0b10000010;

#define EVA_RAW_STATE_TOPIC "RAW/"
#define EVA_LOCAL_STATE_TOPIC "ST/LOC/"
#define EVA_REMOTE_STATE_TOPIC "ST/REM/"
#define EVA_REMOTE_ARCHIVE_STATE_TOPIC "ST/RAR/"
#define EVA_ANY_STATE_TOPIC "ST/+/"
#define EVA_REPLICATION_STATE_TOPIC "RPL/ST/"
#define EVA_REPLICATION_INVENTORY_TOPIC "RPL/INVENTORY/"
#define EVA_REPLICATION_NODE_STATE_TOPIC "RPL/NODE/"
#define EVA_LOG_INPUT_TOPIC "LOG/IN/"
#define EVA_LOG_EVENT_TOPIC "LOG/EV/"
#define EVA_LOG_CALL_TRACE_TOPIC "LOG/TR/"
#define EVA_SERVICE_STATUS_TOPIC "SVC/ST"
#define EVA_AAA_ACL_TOPIC "AAA/ACL/"
#define EVA_AAA_KEY_TOPIC "AAA/KEY/"
#define EVA_AAA_USER_TOPIC "AAA/USER/"
#define EVA_ACTION_STATUS_TOPIC "ACT/"

#endif