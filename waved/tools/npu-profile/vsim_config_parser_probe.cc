// Isolated host-only probe for StateParser's five-field config selector.
//
// This program is intentionally narrow:
//   * it loads only the reviewed x86 libEmulator.so;
//   * it resolves three StateParser selection/copy methods;
//   * it uses a zeroed 0x200-byte fake receiver whose only initialized member
//     is the statically proven default-target ID at offset 0xf0;
//   * it never resolves or calls OpenVX, VSC, CLC, graph, or compiler symbols.
//
// Run it only through compiler_profile.py's locked-down Docker envelope.

#include <array>
#include <cerrno>
#include <cstdint>
#include <cstdio>
#include <cstring>
#include <dlfcn.h>
#include <fcntl.h>
#include <iomanip>
#include <iostream>
#include <sstream>
#include <string>
#include <string_view>
#include <unistd.h>

namespace {

constexpr std::size_t kConfigNameBytes = 80;
constexpr std::size_t kFakeStateParserBytes = 0x200;
constexpr std::size_t kDefaultTargetOffset = 0xf0;
constexpr std::uint32_t kDefaultTargetId = 187;

struct VSimulatorConfig {
  char name[kConfigNameBytes];
  std::uint32_t chip_model;
  std::uint32_t chip_revision;
  std::uint32_t product_id;
  std::uint32_t eco_id;
  std::uint32_t customer_id;
};

static_assert(sizeof(VSimulatorConfig) == 100);

using ChooseByConfigFile = void (*)(void *, const char *);
using ChooseByName = void (*)(void *, const char *);
using GetConfig = void (*)(void *, VSimulatorConfig *);

std::string json_escape(std::string_view value) {
  std::ostringstream output;
  for (const unsigned char character : value) {
    switch (character) {
      case '"':
        output << "\\\"";
        break;
      case '\\':
        output << "\\\\";
        break;
      case '\b':
        output << "\\b";
        break;
      case '\f':
        output << "\\f";
        break;
      case '\n':
        output << "\\n";
        break;
      case '\r':
        output << "\\r";
        break;
      case '\t':
        output << "\\t";
        break;
      default:
        if (character < 0x20 || character >= 0x7f) {
          output << "\\u00" << std::hex << std::setw(2)
                 << std::setfill('0') << static_cast<unsigned>(character)
                 << std::dec;
        } else {
          output << static_cast<char>(character);
        }
    }
  }
  return output.str();
}

std::string canonical_config(
    std::uint32_t revision = 0x00009001,
    std::uint32_t customer = 0x1000000e) {
  char buffer[512] = {};
  const int written = std::snprintf(
      buffer,
      sizeof(buffer),
      "chipModel              = 0x00009000;\n"
      "chipRevision           = 0x%08x;\n"
      "productID              = 0x05090001;\n"
      "ecoID                  = 0x04000000;\n"
      "customerID             = 0x%08x;\n",
      revision,
      customer);
  if (written < 0 || static_cast<std::size_t>(written) >= sizeof(buffer)) {
    return {};
  }
  return std::string(buffer, static_cast<std::size_t>(written));
}

bool prepare_case(
    std::string_view case_name,
    std::string *operation,
    std::string *argument,
    std::string *fixture) {
  *operation = "file";
  fixture->clear();
  if (case_name == "built-in-wave-name") {
    *operation = "name";
    *argument = "VIP9000NANOS_PID0X1000000E";
    return true;
  }
  if (case_name == "valid-database") {
    *fixture = canonical_config();
  } else if (case_name == "valid-live") {
    *fixture = canonical_config(0x00009002);
  } else if (case_name == "missing") {
    *argument = "/tmp/wave-vsim-probe-deliberately-missing.config";
    return true;
  } else if (case_name == "empty") {
    fixture->clear();
  } else if (case_name == "truncated") {
    *fixture = "chipModel              = 0x00009000;\n";
  } else if (case_name == "wrong-order") {
    *fixture =
        "chipRevision           = 0x00009001;\n"
        "chipModel              = 0x00009000;\n"
        "productID              = 0x05090001;\n"
        "ecoID                  = 0x04000000;\n"
        "customerID             = 0x1000000e;\n";
  } else if (case_name == "duplicate-key") {
    *fixture =
        "chipModel              = 0x00009000;\n"
        "chipModel              = 0x00009000;\n"
        "productID              = 0x05090001;\n"
        "ecoID                  = 0x04000000;\n"
        "customerID             = 0x1000000e;\n";
  } else if (case_name == "unknown-key") {
    *fixture =
        "chipModel              = 0x00009000;\n"
        "chipRevision           = 0x00009001;\n"
        "productID              = 0x05090001;\n"
        "ecoID                  = 0x04000000;\n"
        "unknownID              = 0x1000000e;\n";
  } else if (case_name == "malformed-number") {
    *fixture =
        "chipModel              = nope;\n"
        "chipRevision           = 0x00009001;\n"
        "productID              = 0x05090001;\n"
        "ecoID                  = 0x04000000;\n"
        "customerID             = 0x1000000e;\n";
  } else if (case_name == "overlong-line") {
    *fixture = "chipModel";
    fixture->append(300, ' ');
    fixture->append(
        "= 0x00009000;\n"
        "chipRevision           = 0x00009001;\n"
        "productID              = 0x05090001;\n"
        "ecoID                  = 0x04000000;\n"
        "customerID             = 0x1000000e;\n");
  } else if (case_name == "wrong-customer") {
    *fixture = canonical_config(0x00009001, 0xdeadbeef);
  } else if (case_name == "trailing-content") {
    *fixture = canonical_config();
    fixture->append("ignoredTrailingKey = 0x1;\n");
  } else {
    return false;
  }

  char path[] = "/tmp/wave-vsim-probe-XXXXXX";
  const int descriptor = ::mkstemp(path);
  if (descriptor < 0) {
    std::perror("mkstemp");
    return false;
  }
  std::size_t offset = 0;
  while (offset < fixture->size()) {
    const ssize_t count = ::write(
        descriptor, fixture->data() + offset, fixture->size() - offset);
    if (count < 0) {
      if (errno == EINTR) {
        continue;
      }
      std::perror("write");
      ::close(descriptor);
      ::unlink(path);
      return false;
    }
    offset += static_cast<std::size_t>(count);
  }
  if (::close(descriptor) != 0) {
    std::perror("close");
    ::unlink(path);
    return false;
  }
  *argument = path;
  return true;
}

template <typename Function>
Function resolve(void *library, const char *symbol) {
  ::dlerror();
  void *address = ::dlsym(library, symbol);
  const char *error = ::dlerror();
  if (error != nullptr || address == nullptr) {
    std::cerr << "dlsym failed for " << symbol << ": "
              << (error == nullptr ? "null address" : error) << '\n';
    return nullptr;
  }
  return reinterpret_cast<Function>(address);
}

std::string hex32(std::uint32_t value) {
  std::ostringstream output;
  output << "0x" << std::hex << std::setw(8) << std::setfill('0') << value;
  return output.str();
}

}  // namespace

int main(int argc, char **argv) {
  if (argc != 3) {
    std::cerr << "usage: vsim_config_parser_probe LIBEMULATOR CASE\n";
    return 64;
  }
  const std::string library_path = argv[1];
  const std::string case_name = argv[2];

  std::string operation;
  std::string argument;
  std::string fixture;
  if (!prepare_case(case_name, &operation, &argument, &fixture)) {
    std::cerr << "unknown or unpreparable fixed case: " << case_name << '\n';
    return 65;
  }

  void *library = ::dlopen(library_path.c_str(), RTLD_NOW | RTLD_LOCAL);
  if (library == nullptr) {
    std::cerr << "dlopen failed: " << ::dlerror() << '\n';
    if (operation == "file" && case_name != "missing") {
      ::unlink(argument.c_str());
    }
    return 66;
  }
  const auto choose_file = resolve<ChooseByConfigFile>(
      library, "_ZN11StateParser24chooseTargetByConfigFileEPKc");
  const auto choose_name = resolve<ChooseByName>(
      library, "_ZN11StateParser18chooseTargetByNameEPKc");
  const auto get_config = resolve<GetConfig>(
      library, "_ZN11StateParser9getConfigEP17_VSimulatorConfig");
  if (choose_file == nullptr || choose_name == nullptr ||
      get_config == nullptr) {
    ::dlclose(library);
    if (operation == "file" && case_name != "missing") {
      ::unlink(argument.c_str());
    }
    return 67;
  }

  alignas(16) std::array<unsigned char, kFakeStateParserBytes> state{};
  std::uint32_t default_target = kDefaultTargetId;
  std::memcpy(
      state.data() + kDefaultTargetOffset,
      &default_target,
      sizeof(default_target));

  if (operation == "name") {
    choose_name(state.data(), argument.c_str());
  } else {
    choose_file(state.data(), argument.c_str());
  }

  VSimulatorConfig config{};
  get_config(state.data(), &config);
  if (operation == "file" && case_name != "missing") {
    ::unlink(argument.c_str());
  }
  std::fflush(nullptr);

  const std::size_t name_length = ::strnlen(config.name, sizeof(config.name));
  const std::string name(config.name, name_length);
  std::cout
      << "PROBE_RESULT {"
      << "\"case\":\"" << json_escape(case_name) << "\","
      << "\"operation\":\"" << json_escape(operation) << "\","
      << "\"name\":\"" << json_escape(name) << "\","
      << "\"chip_model\":\"" << hex32(config.chip_model) << "\","
      << "\"chip_revision\":\"" << hex32(config.chip_revision) << "\","
      << "\"product_id\":\"" << hex32(config.product_id) << "\","
      << "\"eco_id\":\"" << hex32(config.eco_id) << "\","
      << "\"customer_id\":\"" << hex32(config.customer_id) << "\""
      << "}\n";
  std::cout.flush();
  ::dlclose(library);
  return 0;
}
