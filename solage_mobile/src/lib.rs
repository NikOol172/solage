use solage_core::{PlatformBackend, NoAuth};
use solage_ui::SolageApp;
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::mpsc::Sender;

#[cfg(target_os = "android")]
use android_activity::AndroidApp;

// NOUVEAU : Une variable globale pour stocker notre canal le temps que l'utilisateur choisisse son fichier
static FILE_SENDER: Mutex<Option<Sender<(String, String)>>> = Mutex::new(None);

struct MobileBackend {
    data_dir: PathBuf,
    #[cfg(target_os = "android")]
    app: AndroidApp, // NOUVEAU : On garde une référence à l'application Android pour le JNI
}

impl PlatformBackend for MobileBackend {

    fn save_file(&self, _path: &PathBuf, _content: &str) -> Result<(), String> {
        Err("Non supporté en accès direct".to_string())
    }

    fn launch_external(&self, _cmd: &str, _args: &[&str]) -> Result<(), String> {
        Ok(())
    }

    fn get_config_dir(&self) -> PathBuf {
        self.data_dir.clone()
    }

    // NOUVEAU : L'implémentation asynchrone mobile !
    fn pick_file_async_mobile(&self, tx: std::sync::mpsc::Sender<(String, String)>) {
        #[cfg(target_os = "android")]
        {
            // 1. On sauvegarde l'émetteur (tx) pour la réponse de Java
            if let Ok(mut sender_guard) = FILE_SENDER.lock() {
                *sender_guard = Some(tx);
            }

            // 2. On récupère l'environnement JNI
            let vm_ptr = self.app.vm_as_ptr() as *mut jni::sys::JavaVM;
            let activity_ptr = self.app.activity_as_ptr() as jni::sys::jobject;

            // 3. On appelle la méthode "openFilePicker" sur notre MainActivity Java/Kotlin
            unsafe {
                if let Ok(jvm) = jni::JavaVM::from_raw(vm_ptr) {
                    if let Ok(mut env) = jvm.attach_current_thread() {

                        let activity_obj = jni::objects::JObject::from_raw(activity_ptr);

                        let _ = env.call_method(
                            activity_obj,
                            "openFilePicker",
                            "()V", // "V" signifie que la fonction Java retourne void (rien)
                            &[]
                        );
                    }
                }
            }
        }
    }
}

// --- CALLBACK JNI : Fonction appelée par Android/Java une fois le fichier sélectionné ---
#[cfg(target_os = "android")]
#[unsafe(no_mangle)]
pub extern "system" fn Java_com_cloudcompositing_solage_MainActivity_onFileSelected(
    mut env: jni::JNIEnv,
    _class: jni::objects::JClass,
    name: jni::objects::JString,
    content: jni::objects::JString,
) {
    // 1. On convertit les chaînes Java en chaînes Rust
    if let (Ok(name_str), Ok(content_str)) = (env.get_string(&name), env.get_string(&content)) {
        let file_name: String = name_str.into();
        let file_content: String = content_str.into();

        // 2. On récupère notre canal (Sender) mis en attente et on envoie les données à egui !
        if let Ok(mut sender_guard) = FILE_SENDER.lock() {
            if let Some(tx) = sender_guard.take() {
                let _ = tx.send((file_name, file_content));
            }
        }
    }
}

// --- POINT D'ENTRÉE ANDROID ---
#[cfg(target_os = "android")]
#[unsafe(no_mangle)]
fn android_main(app: AndroidApp) {
    use log::LevelFilter;

    android_logger::init_once(
        android_logger::Config::default()
            .with_max_level(LevelFilter::Info)
            .with_tag("SOLAGE")
    );
    
    log::info!("=== SOLAGE ANDROID DÉMARRAGE ===");

    let data_dir = app.internal_data_path().unwrap_or_else(|| PathBuf::from("/data/local/tmp"));

    let options = eframe::NativeOptions {
        android_app: Some(app.clone()),
        ..Default::default()
    };

    eframe::run_native(
        "Solage Mobile",
        options,
        Box::new(move |cc| {
            // NOUVEAU : On passe l'app (qui contient le pointeur JNI) au Backend
            let backend = MobileBackend { data_dir, app: app.clone() };
            let mut solage_app = SolageApp::new(cc, Box::new(backend), Box::new(NoAuth::new()));
            Ok(Box::new(solage_app))
        }),
    ).unwrap();
}
