#include <eva4-ffi-sdk.hpp>

#include <unistd.h>
#include <iostream>
#include <chrono>
#include <thread>

using namespace std;

string AUTHOR = "Bohemia Automation Limited";
string VERSION = "0.0.1";
string DESCRIPTION = "Example FFI service";

struct Config {
  double a = 0;
  string b;

  MSGPACK_DEFINE_MAP(a, b);
};

eva::vars::Initial<Config> initial;

thread worker_thread;
chrono::milliseconds timeout;

eva::OID unit1 = eva::OID("unit:test");
atomic<int16_t> unit1_value(0);

eva::OID sensor1 = eva::OID("sensor:test");

void worker() {

  try {
    eva::waitCore(timeout);
  } catch(eva::Exception &e) {
    eva::poc(e.what());
  }

  double sensor_value = 0.0;

  while(eva::active()) {
    sensor1.setState(sensor_value);
    int16_t uval = unit1_value;
    unit1.setState(uval);

    eva::log::info("alive");
    this_thread::sleep_for(chrono::seconds(1));
    sensor_value++;
  }
  eva::log::warn("worker terminating");
}

struct PayloadHello {
  string name;

  MSGPACK_DEFINE_MAP(name);
};

extern "C" {

  // process bus frames
  void svc_on_frame(struct EvaFFIFrame *r) {
    eva::Frame frame = eva::Frame(r);
    cout << "Got frame, sender: " << frame.primary_sender() << ", topic: " << frame.topic() << endl;
    if (frame.hasPayload()) {
      auto oh = frame.unpack();
      msgpack::object const& obj = oh.get();
      cout << "payload: " << obj << endl << flush;
    }
  }

  // process bus RPC calls
  int32_t svc_on_rpc_call(struct EvaFFIRpcEvent *r) {
    eva::RpcEvent ev = eva::RpcEvent(r);
    string method = ev.parse_method();
    cout << "Got RPC call, sender: " << ev.primary_sender() << ", method: " << method << endl;
    if (method == "action") {
      // an example for unit actions
      // get action oid first if need to select what value type to parse
      auto oid = ev.asUnitActionOID();
      eva::log::info(oid.i);
      // get the full action
      auto a = ev.asUnitAction<int16_t>();
      a.markPending();
      // process action in the same thread (in production actions are usually put into a queue)
      a.markRunning();
      if (a.oid == unit1) {
        unit1_value = a.params.value;
        a.markCompleted("all fine");
        // the unit state will be updated by the worker
      } else {
        a.markFailed("all bad", -1);
        a.oid.markError();
      }
      return EVA_OK;
    } else if (method == "hello") {
      // responds to hello worh params name=NAME or without any params
      stringstream res;
      if (ev.hasPayload()) {
        try {
          msgpack::object_handle oh = ev.unpack();
          msgpack::object const& obj = oh.get();
          auto p = obj.as<PayloadHello>();
          if (p.name.empty()) {
            return EVA_ERR_CODE_INVALID_PARAMS;
          }
          res << "hello, " << p.name;
        } catch(...) {
          return EVA_ERR_CODE_INVALID_PARAMS;
        }
      } else {
        res << "hi there";
      }
      return eva::result(res.str());
    } else if (method == "bye") {
      // example, terminates the service (does the same as the default "stop" method)
      eva::terminate();
      return EVA_OK;
    } else if (method == "crash") {
      // example, stops the service with a panic on a critical event
      eva::poc("by request");
      return EVA_OK;
    } else {
      // return error code if no supported method found
      return EVA_ERR_CODE_METHOD_NOT_FOUND;
    }
  }

  // mandatory, no bus functions allowed
  int32_t svc_init(struct EvaFFIBuffer *initial_buf) {
    try {
      // try to make bus op when not ready yet
      eva::subscribe("#");
    } catch (eva::Exception &e) {
      // catch exception and log it
      e.log("init subscribe");
      // equal to
      //cerr << e.what() << endl << flush;
    }
    // unpack the initial payload
    try {
      initial = eva::unpackInitial<Config>(initial_buf);
      timeout = initial.getTimeout();
    } catch (exception &e) {
      cerr << e.what() << endl << flush;
      return EVA_ERR_CODE_INVALID_PARAMS;
    }

    cout << "Config loaded: a = " << initial.config.a << ", b = " << initial.config.b << endl << flush;

    // return service info
    eva::ServiceInfo svc_info = eva::ServiceInfo(AUTHOR, VERSION, DESCRIPTION)
      .addMethod(eva::ServiceMethod("hello").optional("name"))
      .addMethod(eva::ServiceMethod("bye"))
      .addMethod(eva::ServiceMethod("crash"));
    return eva::result(svc_info);
  }

  // called before the service is marked as ready, bus connection active
  int16_t svc_prepare() {
    eva::log::info("doing preparing stuff");
    // subscribe to item states (in this example - to all items)
    //eva::subscribe(string(EVA_ANY_STATE_TOPIC) + "#");
    return EVA_OK;
  }

  // calling after the service is marked as ready
  int16_t svc_launch() {
    eva::log::info("the service is ready");
    worker_thread = thread(worker);
    // example API calls
    if (eva::coreActive()) {
      {
        auto res = eva::rpcCall("eva.core", "test");
        if (res.hasPayload()) {
          auto oh = res.unpack();
          msgpack::object const& obj = oh.get();
          cout << obj << endl << flush;
        } else {
          cout << res.code << endl << flush;
        }
      }
      {
        auto res = eva::rpcCall("eva.core", "item.state", eva::CallParamsId { sensor1.i });
        if (res.hasPayload()) {
          auto oh = res.unpack();
          msgpack::object const& obj = oh.get();
          cout << obj << endl << flush;
        } else {
          cout << res.code << endl << flush;
        }
      }
    }
    return EVA_OK;
  }

  // called after the service is marked as terminating
  int16_t svc_terminate() {
    eva::log::warn("the service is terminating");
    worker_thread.join();
    return EVA_OK;
  }

}
