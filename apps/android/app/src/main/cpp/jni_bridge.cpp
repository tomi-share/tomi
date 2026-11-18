#include <jni.h>

#include <string>
#include <vector>

#include "highlight_core.h"

// eu.shaffe.tomi.ui.code.ExtraGrammarNative.tokenize(code: ByteArray, language: String): IntArray
// Flat triplets (start, end, tokenType), offsets in UTF-16 code units of the String that [code] was encoded from. Empty on any failure.
extern "C" JNIEXPORT jintArray JNICALL Java_eu_shaffe_tomi_ui_code_ExtraGrammarNative_tokenize(JNIEnv* env, jobject /* this */,
                                                                                             jbyteArray code, jstring language) {
    jintArray empty = env->NewIntArray(0);
    if (code == nullptr || language == nullptr) return empty;

    const char* languageChars = env->GetStringUTFChars(language, nullptr);
    if (languageChars == nullptr) return empty;
    const std::string name(languageChars);
    env->ReleaseStringUTFChars(language, languageChars);

    tomi::Language lang;
    if (name == "php") {
        lang = tomi::Language::Php;
    } else {
        return empty;
    }

    const jsize length = env->GetArrayLength(code);
    if (length <= 0 || static_cast<size_t>(length) > tomi::kMaxBytes) return empty;
    std::vector<char> bytes(static_cast<size_t>(length));
    env->GetByteArrayRegion(code, 0, length, reinterpret_cast<jbyte*>(bytes.data()));

    std::vector<int32_t> tokens;
    if (!tomi::tokenize(lang, bytes.data(), bytes.size(), tokens) || tokens.empty()) return empty;

    jintArray result = env->NewIntArray(static_cast<jsize>(tokens.size()));
    if (result == nullptr) return empty;
    env->SetIntArrayRegion(result, 0, static_cast<jsize>(tokens.size()), reinterpret_cast<const jint*>(tokens.data()));
    return result;
}
