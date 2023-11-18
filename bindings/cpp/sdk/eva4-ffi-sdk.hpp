#ifndef EVA4_FFI_SDK_HPP
#define EVA4_FFI_SDK_HPP

#include <iostream>
#include <chrono>
#include <thread>
#include <msgpack.hpp>
#include "eva4-common.h"
#include "eva4-ffi.h"

namespace eva {

  const uint16_t ABI_VERSION = 1;

  using namespace std;

  namespace vars {

    using namespace std;

    const auto sleepStep = chrono::milliseconds(100);

    struct TimeoutConfig {
      chrono::milliseconds startup;
      chrono::milliseconds shutdown;
      chrono::milliseconds _default;

      void msgpack_unpack(msgpack::object o) {
        if (o.type != msgpack::type::MAP) {
          throw msgpack::type_error();
        }
        msgpack::object_kv* p = o.via.map.ptr;
        msgpack::object_kv* const pend = o.via.map.ptr + o.via.map.size;

        for (; p != pend; ++p) {
          if (p->key.as<std::string>() == "startup") {
            startup = chrono::milliseconds((int32_t)(p->val.as<double>() * 1000));
          } else if (p->key.as<std::string>() == "shutdown") {
            shutdown = chrono::milliseconds((int32_t)(p->val.as<double>() * 1000));
          } else if (p->key.as<std::string>() == "default") {
            _default = chrono::milliseconds((int32_t)(p->val.as<double>() * 1000));
          }
        }
      }
    };

    struct CoreInfo {
      uint64_t build;
      string version;
      uint16_t eapi_version;
      string path;
      bool active;

      MSGPACK_DEFINE_MAP(build, version, eapi_version, path, active);
    };

    template<typename T>struct Initial {
      uint16_t version;
      string system_name;
      string id;
      string data_path;
      TimeoutConfig timeout;
      CoreInfo core;
      T config;
      string user;
      bool fail_mode;
      bool fips;
      bool call_tracing;

      MSGPACK_DEFINE_MAP(version, system_name, id, data_path,
          timeout, core, config, user, fail_mode, fips, call_tracing);

      chrono::milliseconds getTimeout() {
        return this->timeout._default;
      }

      string evaDir() {
        return this->core.path;
      }
    };

    enum ItemKind {
      Unit,
      Sensor,
      LVar,
      LMacro,
      Any
    };

    struct errorMapStr : public map<int16_t, string>
    {
      errorMapStr()
      {
        this->operator[](-32001) = "Not found";
        this->operator[](-32002) = "Access denied";
        this->operator[](-32003) = "System error";
        this->operator[](-32004) = "Other";
        this->operator[](-32005) = "Not ready";
        this->operator[](-32006) = "Unsupported";
        this->operator[](-32007) = "Core error";
        this->operator[](-32008) = "Timeout";
        this->operator[](-32009) = "Invalid data";
        this->operator[](-32010) = "Function failed";
        this->operator[](-32011) = "Aborted";
        this->operator[](-32012) = "Resource already existS";
        this->operator[](-32013) = "Busy";
        this->operator[](-32014) = "Method not implemented";
        this->operator[](-32015) = "Token restricted";
        this->operator[](-32016) = "I/O";
        this->operator[](-32017) = "Registry";
        this->operator[](-32018) = "evaHI auth required";
        this->operator[](-32022) = "Access denied more data required";
        this->operator[](-32700) = "Parse error";
        this->operator[](-32600) = "Invalid request";
        this->operator[](-32601) = "Method not found";
        this->operator[](-32602) = "Invalid params";
        this->operator[](-32603) = "Internal RPC";
        this->operator[](-32113) = "Bus client not registered";
        this->operator[](-32114) = "Bus data";
        this->operator[](-32115) = "Bus io";
        this->operator[](-32116) = "Bus other";
        this->operator[](-32117) = "Bus not supported";
        this->operator[](-32118) = "Bus busy";
        this->operator[](-32119) = "Bus not delivered";
        this->operator[](-32120) = "Bus timeout";
        this->operator[](-32121) = "Bus access";
      };
    };

    errorMapStr errorMap;

    struct itemKindMap : public map<string, ItemKind>
    {
      itemKindMap()
      {
        this->operator[]("unit") =  Unit;
        this->operator[]("sensor") =  Sensor;
        this->operator[]("lvar") =  LVar;
        this->operator[]("lmacro") =  LMacro;
        this->operator[]("+") =  Any;
      };
    };

    struct itemKindMapStr : public map<ItemKind, string>
    {
      itemKindMapStr()
      {
        this->operator[](Unit) =  "unit";
        this->operator[](Sensor) =  "sensor";
        this->operator[](LVar) =  "lvar";
        this->operator[](LMacro) =  "lmacro";
        this->operator[](Any) =  "+";
      };
    };

    itemKindMap itemMapK;
    itemKindMapStr itemMapS;
  };

  class Exception : public exception {
    public:
      Exception(int16_t code) {
        this->code = code;
        stringstream ss;
        auto i = vars::errorMap.find(code);
        if (i != vars::errorMap.end()) {
          ss << i->second << ' ';
        }
        ss << '(' << this->code << ')';
        this->msg = ss.str();
      }
      char * what() {
        return (char *)this->msg.c_str();
      }
      void log() {
        cerr << this->msg << endl << flush;
      }
      void log(string context) {
        cerr << context << ": " << this->msg << endl << flush;
      }
      int16_t code;
    private:
      string msg;
  };

  struct RawItemStatus {
    int16_t status;

    MSGPACK_DEFINE_MAP(status);
  };

  template <typename T>
    struct RawItemState {
      int16_t status;
      T value;

      MSGPACK_DEFINE_MAP(status, value);
    };

  struct CallParamsId {
    string i;

    MSGPACK_DEFINE_MAP(i);
  };

  atomic<int32_t (*)(int16_t op_code, struct EvaFFIBuffer* payload)> svc_op_fn(nullptr);

  stringstream packStrings(vector<string>& strings) {
    stringstream ss;
    for (const auto& s : strings) {
      ss << s;
      ss.put(0);
    }
    return ss;
  }

  int32_t svcOp(int16_t op_code) {
    return svc_op_fn ? svc_op_fn(op_code, nullptr) : EVA_ERR_CODE_NOT_READY;
  }

  int32_t svcOpSS(int16_t op_code, stringstream& ss) {
    size_t size = ss.tellp();
    char* buf = (char *)malloc(size);
    struct EvaFFIBuffer payload = EvaFFIBuffer { size, buf, size };
    memcpy(payload.data, ss.str().c_str(), size);
    int32_t res = svc_op_fn(op_code, &payload);
    free(buf);
    return res;
  }

  bool active() {
    return svcOp(EVA_FFI_SVC_OP_IS_ACTIVE) == 1;
  }

  void c2e(int16_t code) {
    if (code < EVA_OK)  {
      throw Exception(code);
    }
  }

  /**
   * @throws Exception
   */
  void subscribe(vector<string>& topics) {
    stringstream ss = packStrings(topics);
    c2e(svcOpSS(EVA_FFI_SVC_OP_SUBSCRIBE_TOPIC, ss));
  }

  /**
   * @throws Exception
   */
  void subscribe(string topic) {
    stringstream ss;
    ss << topic;
    c2e(svcOpSS(EVA_FFI_SVC_OP_SUBSCRIBE_TOPIC, ss));
  }

  /**
   * @throws Exception
   */
  void unsubscribe(vector<string>& topics) {
    stringstream ss = packStrings(topics);
    c2e(svcOpSS(EVA_FFI_SVC_OP_UNSUBSCRIBE_TOPIC, ss));
  }

  /**
   * @throws Exception
   */
  void unsubscribe(string topic) {
    stringstream ss;
    ss << topic;
    c2e(svcOpSS(EVA_FFI_SVC_OP_UNSUBSCRIBE_TOPIC, ss));
  }

  /**
   * @throws Exception
   */
  template <typename T> void publish(string topic, T data) {
    stringstream ss;
    ss << topic;
    ss.put(0);
    msgpack::pack(ss, data);
    c2e(svcOpSS(EVA_FFI_SVC_OP_PUBLISH_TOPIC, ss));
  }

  class OID {
    public:
      OID() {
        this->kind = vars::ItemKind::Any;
        this->i = string();
        this->path = string();
        this->rawTopic = string();
      }
      OID(string s) {
        this->parse(s, false);
      }
      OID(string s, bool fromPath) {
        this->parse(s, fromPath);
      }
      string fullId() {
        return this->i.substr(this->pos + 1);
      }
      void markOk() {
        publish(this->rawTopic, RawItemStatus{ EVA_ITEM_STATUS_OK });
      }
      void markError() {
        publish(this->rawTopic, RawItemStatus{ EVA_ITEM_STATUS_ERROR });
      }
      template<typename T> void setState(T value) {
        publish(this->rawTopic, RawItemState<T>{ EVA_ITEM_STATUS_OK, value });
      }
      vars::ItemKind kind;
      string i;
      string path;
      string rawTopic;
      bool operator==(const OID& other) const {
        return this->i==other.i;
      }
    private:
      void parse(string s, bool fromPath) {
        char sep = fromPath?'/':':';
        size_t pos = s.find(sep);
        if (pos == string::npos) {
          string opk = fromPath?"oid path":"oid";
          throw invalid_argument("invalid " + opk + ": " + s);
        }
        string sKind = s.substr(0, pos);
        auto i = vars::itemMapK.find(sKind);
        if (i == vars::itemMapK.end()) {
          throw invalid_argument("invalid oid kind: " + s);
        }
        string full_id = s.substr(pos + 1);
        this->kind = i->second;
        this->i = fromPath?sKind+":"+full_id:s;
        this->path = fromPath?s:sKind+"/"+full_id;
        this->rawTopic = EVA_RAW_STATE_TOPIC + this->path;
      }
      size_t pos;
  };

  ostream& operator<<(ostream &strm, const OID &o) {
    return strm << o.i;
  }

  thread_local stringstream result_buf;

  size_t rp() {
    return result_buf.tellp();
  }

  namespace controller {

    using namespace std;

    typedef uint8_t uuidBuf[16];

    struct BusActionStatusShort {
      uint8_t status;
      uuidBuf uuid;

      MSGPACK_DEFINE_MAP(uuid, status)
    };

    struct BusActionStatusCompleted {
      uint8_t status;
      string out;
      int8_t exitcode;
      uuidBuf uuid;

      MSGPACK_DEFINE_MAP(uuid, status, out, exitcode)
    };

    struct BusActionStatusError {
      uint8_t status;
      string err;
      int8_t exitcode;
      uuidBuf uuid;

      MSGPACK_DEFINE_MAP(uuid, status, err, exitcode)
    };

    template <typename V> struct UnitActionParams {
      V value;

      MSGPACK_DEFINE_MAP(value)
    };

    template <typename T> struct ActionData {
      uuidBuf uuid;
      string i;
      uint32_t timeout;
      uint8_t priority;
      T params;

      MSGPACK_DEFINE_MAP(uuid,i,timeout,priority,params)
    };

    struct ActionDataOID {
      string i;

      MSGPACK_DEFINE_MAP(i)
    };

    template <typename T> class Action {
      public:
        Action(ActionData<T> a) {
          copy(begin(a.uuid), end(a.uuid), begin(this->uuid));
          this->oid = a.i;
          this->timeout = chrono::microseconds(a.timeout);
          this->priority = a.priority;
          this->params = a.params;
          this->topic = string(EVA_ACTION_STATUS_TOPIC) + this->oid.path;
        }
        void markPending() {
          this->markAction(EVA_ACTION_STATUS_PENDING);
        }
        void markRunning() {
          this->markAction(EVA_ACTION_STATUS_RUNNING);
        }
        void markCompleted(string out) {
          BusActionStatusCompleted a = { EVA_ACTION_STATUS_COMPLETED, out, 0 };
          copy(begin(this->uuid), end(this->uuid), begin(a.uuid));
          publish<BusActionStatusCompleted>(this->topic, a);
        }
        void markFailed(string err, int8_t exitcode) {
          BusActionStatusError a { EVA_ACTION_STATUS_FAILED, err, exitcode };
          copy(begin(this->uuid), end(this->uuid), begin(a.uuid));
          publish<BusActionStatusError>(this->topic, a);
        }
        void markCanceled() {
          this->markAction(EVA_ACTION_STATUS_CANCELED);
        }
        void markTerminated() {
          this->markAction(EVA_ACTION_STATUS_TERMINATED);
        }
        uuidBuf uuid;
        OID oid;
        chrono::microseconds timeout;
        uint8_t priority;
        T params;
      private:
        string topic;
        void markAction(uint8_t status) {
          BusActionStatusShort a { status };
          copy(begin(this->uuid), end(this->uuid), begin(a.uuid));
          publish<BusActionStatusShort>(this->topic, a);
        }
    };

  };

  struct SvcMethodParam {
    bool required;

    MSGPACK_DEFINE_MAP(required);
  };

  struct SvcMethod {
    string description;
    map<string, SvcMethodParam> params;

    MSGPACK_DEFINE_MAP(description, params);
  };

  class ServiceMethod {
    public:
      ServiceMethod(string name) {
        this->name = name;
        this->description= "";
      }
      ServiceMethod(string name, string description) {
        this->name = name;
        this->description= description;
      }
      ServiceMethod required(string name) {
        this->params.insert(pair<string, SvcMethodParam>(name, SvcMethodParam { required: true }));
        return *this;
      }
      ServiceMethod optional(string name) {
        this->params.insert(pair<string, SvcMethodParam>(name, SvcMethodParam { required: false }));
        return *this;
      }
      string name;
      string description;
      map<string, SvcMethodParam> params;
  };

  struct ServiceInfo {
    ServiceInfo(string author, string version, string description) {
      this->author = author;
      this->version = version;
      this->description = description;
    }
    ServiceInfo addMethod(ServiceMethod method) {
      methods.insert(pair<string, SvcMethod>(method.name, SvcMethod {method.description, method.params}));
      return *this;
    }
    string author;
    string version;
    string description;
    map<string, SvcMethod> methods;

    MSGPACK_DEFINE_MAP(author, version, description, methods);
  };

  template <typename T> vars::Initial<T> unpackInitial(EvaFFIBuffer *buf) {
    msgpack::object_handle oh = msgpack::unpack(buf->data, buf->len);
    msgpack::object const& obj = oh.get();
    auto p = obj.as<vars::Initial<T>>();
    return p;
  }

  class Frame {
    public:
      Frame(EvaFFIFrame* r) {
        this->r = r;
      }
      string primary_sender() {
        return string(this->r->primary_sender);
      }
      string topic() {
        return string(this->r->topic);
      }
      bool hasPayload() {
        return this->r->payload_size > 0;
      }
      msgpack::object_handle unpack() {
        msgpack::object_handle oh = msgpack::unpack(this->r->payload, this->r->payload_size);
        return oh;
      }
    private:
      EvaFFIFrame* r;
  };

  class RpcEvent {
    public:
      RpcEvent(EvaFFIRpcEvent* r) {
        this->r = r;
      }
      string primary_sender() {
        return string(this->r->primary_sender);
      }
      string parse_method() {
        return string(this->r->method, this->r->method_size);
      }
      bool hasPayload() {
        return this->r->payload_size > 0;
      }
      msgpack::object_handle unpack() {
        msgpack::object_handle oh = msgpack::unpack(this->r->payload, this->r->payload_size);
        return oh;
      }
      OID asUnitActionOID() {
        if (this->hasPayload()) {
          msgpack::object_handle oh=this->unpack();
          msgpack::object const& obj = oh.get();
          auto a = obj.as<controller::ActionDataOID>();
          return OID(a.i);
        } else {
          throw Exception(EVA_ERR_CODE_INVALID_PARAMS);
        }
      }
      template <typename T>controller::Action<controller::UnitActionParams<T>> asUnitAction() {
        if (this->hasPayload()) {
          msgpack::object_handle oh=this->unpack();
          msgpack::object const& obj = oh.get();
          auto a = obj.as<controller::ActionData<controller::UnitActionParams<T>>>();
          return a;
        } else {
          throw Exception(EVA_ERR_CODE_INVALID_PARAMS);
        }
      }
    private:
      EvaFFIRpcEvent* r;
  };

  class RpcResult {
    public:
      int16_t code;

      RpcResult(int32_t res) {
        if (res > 0) {
          size_t size = (size_t)res;
          char* data = (char *)malloc(size);
          this->buf = EvaFFIBuffer { size, data, size };
          this->code = 0;
          this->size = size;
          this->getCallResult();
          this->data = data;
        } else {
          this->code = (int16_t)res;
          this->size = 0;
        }
      }
      bool hasPayload() {
        return this->size > 0;
      }
      msgpack::object_handle unpack() {
        msgpack::object_handle oh = msgpack::unpack(this->buf.data, this->buf.len);
        return oh;
      }
      ~RpcResult() {
        free(this->data);
      }
      size_t size;
      EvaFFIBuffer buf;
    private:
      void getCallResult() {
        int16_t code = svc_op_fn(EVA_FFI_SVC_OP_GET_RPC_RESULT, &this->buf);
        if (code != EVA_OK) {
          this->size = 0;
          this->code = EVA_ERR_CODE_CORE_ERROR;
        }
      }
      char* data;
  };

  /**
   * @throws Exception
   */
  template <typename T> RpcResult rpcCall(string target, string method, T params) {
    stringstream ss;
    ss << target;
    ss.put(0);
    ss << method;
    ss.put(0);
    msgpack::pack(ss, params);
    int32_t res = svcOpSS(EVA_FFI_SVC_OP_RPC_CALL, ss);
    c2e(res);
    RpcResult rpc_result = RpcResult(res);
    return rpc_result;
  }

  /**
   * @throws Exception
   */
  RpcResult rpcCall(string target, string method) {
    stringstream ss;
    ss << target;
    ss.put(0);
    ss << method;
    int32_t res = svcOpSS(EVA_FFI_SVC_OP_RPC_CALL, ss);
    c2e(res);
    RpcResult rpc_result = RpcResult(res);
    return rpc_result;
  }

  /**
   * @throws Exception
   */
  template<typename T> int32_t result(T payload) {
    msgpack::pack(result_buf, payload);
    return result_buf.tellp();
  }

  /**
   * @throws Exception
   */
  void terminate() {
    c2e(svcOp(EVA_FFI_SVC_OP_TERMINATE));
  }

  /**
   * @throws Exception
   */
  void poc() {
    c2e(svcOp(EVA_FFI_SVC_OP_POC));
  }

  struct coreStatus {
    bool active;

    MSGPACK_DEFINE_MAP(active);
  };

  /**
   * @throws Exception
   */
  bool coreActive() {
    RpcResult r = rpcCall("eva.core", "test");
    msgpack::object_handle oh = r.unpack();
    msgpack::object const& obj = oh.get();
    auto p = obj.as<coreStatus>();
    return p.active;
  }

  /**
   * @throws Exception
   */
  void waitCore() {
    while (!coreActive()) {
      this_thread::sleep_for(vars::sleepStep);
    }
  }

  /**
   * @throws Exception
   */
  void waitCore(chrono::milliseconds timeout) {
    const auto waitUntil = chrono::steady_clock::now() + timeout;
    while (!coreActive()) {
      this_thread::sleep_for(vars::sleepStep);
      if (chrono::steady_clock::now() >= waitUntil) {
        throw Exception(EVA_ERR_CODE_TIMEOUT);
      }
    }
  }

  namespace log {

    void trace(string message) {
      stringstream ss;
      ss << message;
      svcOpSS(EVA_FFI_SVC_OP_LOG_TRACE, ss);
    }

    void debug(string message) {
      stringstream ss;
      ss << message;
      svcOpSS(EVA_FFI_SVC_OP_LOG_DEBUG, ss);
    }

    void info(string message) {
      stringstream ss;
      ss << message;
      svcOpSS(EVA_FFI_SVC_OP_LOG_INFO, ss);
    }

    void warn(string message) {
      stringstream ss;
      ss << message;
      svcOpSS(EVA_FFI_SVC_OP_LOG_WARN, ss);
    }

    void error(string message) {
      stringstream ss;
      ss << message;
      svcOpSS(EVA_FFI_SVC_OP_LOG_ERROR, ss);
    }

  }

  namespace internal {

    extern "C" {

      int16_t eva_svc_get_result(struct EvaFFIBuffer *ebuf) {
        size_t size = eva::result_buf.tellp();
        if (size > ebuf->max) return EVA_ERR_CODE_ABORTED;
        ebuf->len = size;
        memcpy(ebuf->data, result_buf.str().c_str(), size);
        result_buf.str(string());
        return EVA_OK;
      }

      int16_t eva_svc_set_op_fn(uint16_t abi_version, int32_t (*f)(int16_t, struct EvaFFIBuffer*)) {
        if (abi_version != ABI_VERSION) {
          return EVA_ERR_CODE_UNSUPPORTED;
        }
        svc_op_fn = f;
        return EVA_OK;
      }

    }

  };

}

#endif
