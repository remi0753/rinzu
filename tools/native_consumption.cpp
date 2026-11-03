// Close callback capability, result aliases and the entire call graph.
#include "llvm/ADT/DenseSet.h"
#include "llvm/Analysis/CaptureTracking.h"
#include "llvm/IR/Instructions.h"
#include "llvm/IR/IntrinsicInst.h"
#include "llvm/IR/Module.h"
#include "llvm/IRReader/IRReader.h"
#include "llvm/Support/JSON.h"
#include "llvm/Support/SourceMgr.h"
#include "llvm/Support/raw_ostream.h"
#include <set>
#include <string>
#include <utility>

class StrictEffects {
  unsigned budget = 10000;
  llvm::DenseSet<std::pair<const llvm::Value *, const llvm::Function *>> done;

public:
  std::set<std::string> reasons;
  unsigned visits = 0;
  void trace(llvm::Value *root, llvm::Function *function) {
    auto key = std::make_pair(root, function);
    if (!done.insert(key).second)
      return;
    for (auto &use : root->uses()) {
      if (budget == 0) {
        reasons.insert("analysis-budget");
        break;
      }
      --budget;
      ++visits;
      auto *instruction = llvm::dyn_cast<llvm::Instruction>(use.getUser());
      if (!instruction) {
        reasons.insert("non-instruction-use");
        continue;
      }
      if (llvm::isa<llvm::GetElementPtrInst, llvm::BitCastInst, llvm::AddrSpaceCastInst,
                    llvm::PHINode, llvm::SelectInst, llvm::FreezeInst>(instruction)) {
        trace(instruction, function);
      } else if (auto *load = llvm::dyn_cast<llvm::LoadInst>(instruction)) {
        if (load->getPointerOperand() != root || load->getType()->isPointerTy())
          reasons.insert("pointer-or-indirect-load");
      } else if (auto *store = llvm::dyn_cast<llvm::StoreInst>(instruction)) {
        reasons.insert(store->getPointerOperand() == root ? "write-original" : "publish-pointer");
      } else if (llvm::isa<llvm::AtomicRMWInst, llvm::AtomicCmpXchgInst>(instruction)) {
        reasons.insert("write-original");
      } else if (auto *compare = llvm::dyn_cast<llvm::ICmpInst>(instruction)) {
        if (compare->getPredicate() != llvm::ICmpInst::ICMP_EQ &&
            compare->getPredicate() != llvm::ICmpInst::ICMP_NE)
          reasons.insert("address-comparison");
      } else if (auto *copy = llvm::dyn_cast<llvm::MemTransferInst>(instruction)) {
        if (copy->getRawSource() != root || copy->getRawDest() == root)
          reasons.insert("write-original");
      } else if (llvm::isa<llvm::MemSetInst>(instruction)) {
        reasons.insert("write-original");
      } else if (auto *call = llvm::dyn_cast<llvm::CallBase>(instruction)) {
        auto *target = call->getCalledFunction();
        if (!target) {
          reasons.insert("indirect-or-asm-consumer");
          continue;
        }
        if (target->getName() == "free" || target->getName().starts_with("_Zdl")) {
          reasons.insert("free-original");
          continue;
        }
        if (target->isDeclaration()) {
          reasons.insert("unknown-body:" + target->getName().str());
          continue;
        }
        if (call->getType()->isPointerTy())
          reasons.insert("pointer-return-helper");
        for (unsigned index = 0; index < call->arg_size(); ++index)
          if (call->getArgOperand(index) == root) {
            if (index >= target->arg_size())
              reasons.insert("varargs-consumer");
            else
              trace(target->getArg(index), target);
          }
      } else {
        reasons.insert("unsupported-use:" + std::string(instruction->getOpcodeName()));
      }
    }
  }
};

class Closed {
  llvm::DenseSet<llvm::Value *> capabilities;
  llvm::DenseSet<llvm::Value *> activeValues;
  llvm::DenseSet<llvm::Function *> activeFunctions, checkedFunctions;
  unsigned budget = 10000;
  StrictEffects effects;

  bool tick() {
    if (!budget) {
      reasons.insert("closure-budget");
      return false;
    }
    --budget;
    return true;
  }
  bool visibleBody(llvm::Function *function) {
    if (!function || function->isDeclaration() || function->isVarArg()) {
      reasons.insert("unknown-or-varargs-body");
      return false;
    }
    if (!function->hasLocalLinkage() && !function->hasHiddenVisibility()) {
      reasons.insert("interposable-body");
      return false;
    }
    return true;
  }
  void capability(llvm::Value *root) {
    if (activeValues.contains(root)) {
      reasons.insert("cyclic-callback-flow");
      return;
    }
    if (!capabilities.insert(root).second)
      return;
    activeValues.insert(root);
    for (auto &use : root->uses()) {
      if (!tick())
        break;
      auto *instruction = llvm::dyn_cast<llvm::Instruction>(use.getUser());
      if (!instruction) {
        reasons.insert("non-instruction-callback-use");
        continue;
      }
      if (llvm::isa<llvm::BitCastInst, llvm::FreezeInst>(instruction)) {
        capability(instruction);
      } else if (llvm::isa<llvm::PHINode, llvm::SelectInst>(instruction)) {
        // A merge can contain an unrelated function pointer. Do not treat the
        // merge as trusted before all incoming capabilities are closed.
        reasons.insert("merged-callback-capability");
      } else if (auto *call = llvm::dyn_cast<llvm::CallInst>(instruction)) {
        if (call->getCalledOperand() == root) {
          if (!call->getType()->isPointerTy()) {
            reasons.insert("non-buffer-callback-result");
            continue;
          }
          ++sites;
          // Actual body walk, plus LLVM capture query; attributes alone never
          // authorize a body-less consumer.
          auto capture = llvm::PointerMayBeCaptured(call, true, llvm::CaptureComponents::All,
                                                    [](llvm::CaptureComponents) { return false; });
          if (llvm::capturesAnything(capture & ~llvm::CaptureComponents::AddressIsNull))
            reasons.insert("result-capture-beyond-nullness");
          effects.trace(call, call->getFunction());
        } else {
          auto *target = call->getCalledFunction();
          if (!visibleBody(target))
            continue;
          for (unsigned index = 0; index < call->arg_size(); ++index)
            if (call->getArgOperand(index) == root) {
              if (index >= target->arg_size())
                reasons.insert("callback-vararg");
              else
                capability(target->getArg(index));
            }
        }
      } else {
        reasons.insert("callback-escape:" + std::string(instruction->getOpcodeName()));
      }
    }
    activeValues.erase(root);
  }
  void body(llvm::Function *function) {
    if (activeFunctions.contains(function)) {
      reasons.insert("recursive-call-graph");
      return;
    }
    if (checkedFunctions.contains(function) || !visibleBody(function))
      return;
    activeFunctions.insert(function);
    for (auto &block : *function)
      for (auto &instruction : block) {
        if (!tick())
          break;
        if (llvm::isa<llvm::InvokeInst, llvm::CallBrInst, llvm::IndirectBrInst, llvm::ResumeInst,
                      llvm::LandingPadInst>(instruction)) {
          reasons.insert("nonlocal-control");
          continue;
        }
        auto *call = llvm::dyn_cast<llvm::CallInst>(&instruction);
        if (!call)
          continue;
        if (capabilities.contains(call->getCalledOperand()))
          continue;
        if (call->isInlineAsm()) {
          reasons.insert("inline-asm");
          continue;
        }
        auto *target = call->getCalledFunction();
        if (target && target->isIntrinsic()) {
          // Restrict to non-calling intrinsics. No arbitrary intrinsic whitelist.
          if (llvm::isa<llvm::MemIntrinsic, llvm::DbgInfoIntrinsic>(call) ||
              target->getIntrinsicID() == llvm::Intrinsic::lifetime_start ||
              target->getIntrinsicID() == llvm::Intrinsic::lifetime_end ||
              target->getIntrinsicID() == llvm::Intrinsic::vector_reduce_add)
            continue;
          reasons.insert("unsupported-intrinsic:" + target->getName().str());
          continue;
        }
        if (!target)
          reasons.insert("unknown-indirect-call");
        else if (target->isDeclaration())
          reasons.insert("unknown-callee:" + target->getName().str());
        else
          body(target);
      }
    activeFunctions.erase(function);
    checkedFunctions.insert(function);
  }

public:
  unsigned sites = 0;
  std::set<std::string> reasons;
  llvm::json::Object analyze(llvm::Function *function, const std::vector<unsigned> &parameters) {
    for (unsigned parameter : parameters) {
      if (parameter >= function->arg_size() ||
          !function->getArg(parameter)->getType()->isPointerTy())
        reasons.insert("invalid-callback-parameter");
      else
        capability(function->getArg(parameter));
    }
    body(function);
    reasons.insert(effects.reasons.begin(), effects.reasons.end());
    if (!sites)
      reasons.insert("no-buffer-result-sites");
    llvm::json::Array failures, positions;
    for (const auto &reason : reasons)
      failures.push_back(reason);
    for (unsigned parameter : parameters)
      positions.push_back(parameter);
    return llvm::json::Object{{"function", function->getName().str()},
                              {"parameters", std::move(positions)},
                              {"result_sites", sites},
                              {"body_use_visits", effects.visits},
                              {"reasons", std::move(failures)},
                              {"closed", reasons.empty()}};
  }
};

int main(int argc, char **argv) {
  if (argc < 2 || argc == 3)
    return 2;
  llvm::LLVMContext context;
  llvm::SMDiagnostic error;
  auto module = llvm::parseIRFile(argv[1], error, context);
  if (!module) {
    error.print(argv[0], llvm::errs());
    return 2;
  }
  llvm::json::Array output;
  if (argc >= 4) {
    auto *function = module->getFunction(argv[2]);
    if (!function || function->isDeclaration())
      return 2;
    std::vector<unsigned> parameters;
    for (int index = 3; index < argc; ++index) {
      unsigned parameter;
      if (llvm::StringRef(argv[index]).getAsInteger(10, parameter))
        return 2;
      parameters.push_back(parameter);
    }
    Closed closed;
    output.push_back(closed.analyze(function, parameters));
  } else {
    for (auto &function : *module) {
      if (function.isDeclaration() || function.arg_empty() ||
          (!function.getName().starts_with("callback_") &&
           !function.getName().starts_with("case_")))
        continue;
      Closed closed;
      output.push_back(closed.analyze(&function, {0}));
    }
  }
  llvm::outs() << llvm::formatv("{0:2}", llvm::json::Value(std::move(output))) << '\n';
}
