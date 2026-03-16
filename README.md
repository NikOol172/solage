# solage

Solage is a declarative GUI framework for Rust that allows you to build native Android, WebAssembly and Desktop applications using:
- **YAML** for layout configuration
- **Rhai** for logic scripting
- **egui** for rendering

Stay tuned for the first release!

Pour résumer l'architecture en béton armé que vous venez de mettre en place pour Solage :

Stabilité système : Un pont JNI qui respecte à 100 % les règles de cycle de vie et de threading de la machine virtuelle Java.

Ergonomie native : Une boîte de dialogue Kotlin qui invoque parfaitement le clavier moderne d'Android 15 et son contrôleur.

Data-Driven : Une file d'attente asynchrone via Mutex qui injecte dynamiquement les valeurs saisies directement dans la bonne cellule de votre hiérarchie YAML (Section -> Mode -> Flavor -> Step).

Rendu Immersif : Un mode plein écran "Edge-to-Edge" sécurisé par des panneaux fantômes pour protéger l'interface des encoches de caméra et des coins arrondis des Pixels.