// The Quest's runtime permission for videos in shared storage. The app is a
// NativeActivity without Java code, so nothing asks for the permission unless
// we call the activity through JNI: without it Android lists Movies, Download
// and the rest as empty folders.
#include <android/api-level.h>
#include <jni.h>

// READ_MEDIA_VIDEO replaced READ_EXTERNAL_STORAGE in Android 13 (API 33);
// the manifest declares both, each for its own range of versions.
static const char *video_permission(void) {
    return android_get_device_api_level() >= 33 ? "android.permission.READ_MEDIA_VIDEO"
                                                : "android.permission.READ_EXTERNAL_STORAGE";
}

// 1 when the app may read videos, 0 when not (then, with `ask`, the system
// dialog is shown; its answer counts from the next call), -1 on a JNI error.
int vz_video_permission(void *vm_ptr, void *activity_ptr, int ask) {
    JavaVM *vm = vm_ptr;
    jobject activity = activity_ptr;
    JNIEnv *env = NULL;
    int attached = 0;
    if ((*vm)->GetEnv(vm, (void **)&env, JNI_VERSION_1_6) == JNI_EDETACHED) {
        if ((*vm)->AttachCurrentThread(vm, &env, NULL) != JNI_OK) return -1;
        attached = 1;
    }
    int result = -1;
    jclass activity_class = (*env)->GetObjectClass(env, activity);
    jstring name = (*env)->NewStringUTF(env, video_permission());
    jmethodID check = (*env)->GetMethodID(env, activity_class, "checkSelfPermission",
                                          "(Ljava/lang/String;)I");
    if (name && check) {
        // PackageManager.PERMISSION_GRANTED is 0.
        jint state = (*env)->CallIntMethod(env, activity, check, name);
        if (!(*env)->ExceptionCheck(env)) result = state == 0;
    }
    if (result == 0 && ask) {
        jmethodID request = (*env)->GetMethodID(env, activity_class, "requestPermissions",
                                                "([Ljava/lang/String;I)V");
        jclass string_class = (*env)->FindClass(env, "java/lang/String");
        jobjectArray names =
            string_class ? (*env)->NewObjectArray(env, 1, string_class, name) : NULL;
        if (request && names) (*env)->CallVoidMethod(env, activity, request, names, 1);
        if (names) (*env)->DeleteLocalRef(env, names);
        if (string_class) (*env)->DeleteLocalRef(env, string_class);
    }
    // A failed request leaves `result` as the check found it.
    if ((*env)->ExceptionCheck(env)) {
        (*env)->ExceptionDescribe(env);
        (*env)->ExceptionClear(env);
    }
    if (name) (*env)->DeleteLocalRef(env, name);
    if (activity_class) (*env)->DeleteLocalRef(env, activity_class);
    if (attached) (*vm)->DetachCurrentThread(vm);
    return result;
}
