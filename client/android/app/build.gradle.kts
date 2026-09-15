plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.plugin.compose")
}

android {
    namespace = "app.turattext.mobile"
    compileSdk = 36
    ndkVersion = "27.2.12479018"

    defaultConfig {
        applicationId = "app.turattext.mobile"
        minSdk = 23
        targetSdk = 36
        versionCode = 30100
        versionName = "3.1.0"
    }

    buildFeatures {
        compose = true
        buildConfig = true
    }

    sourceSets["main"].assets.srcDir("../../../shared/fonts/licenses")

    buildTypes {
        release {
            isMinifyEnabled = true
            isShrinkResources = true
            proguardFiles(getDefaultProguardFile("proguard-android-optimize.txt"), "proguard-rules.pro")
        }
    }

    packaging {
        jniLibs.useLegacyPackaging = true
        resources.excludes += setOf("/META-INF/{AL2.0,LGPL2.1}")
    }
}

dependencies {
    val composeBom = platform("androidx.compose:compose-bom:2026.06.00")
    implementation(composeBom)
    androidTestImplementation(composeBom)

    implementation("androidx.activity:activity-compose:1.13.0")
    implementation("androidx.compose.animation:animation")
    implementation("androidx.compose.material3:material3")
    implementation("androidx.compose.ui:ui")
    implementation("androidx.compose.ui:ui-tooling-preview")
    implementation("androidx.lifecycle:lifecycle-runtime-compose:2.10.0")
    implementation("androidx.lifecycle:lifecycle-viewmodel-compose:2.10.0")
    implementation("org.jetbrains.kotlinx:kotlinx-coroutines-android:1.10.2")
    // Плеер читает видео прямо из зашифрованного вложения через собственный DataSource,
    // поэтому нужен только сам движок: элементы управления рисует Compose.
    implementation("androidx.media3:media3-exoplayer:1.9.0")
    // Перекодирование видео перед отправкой: у платформы нет готового API, а MediaCodec
    // вручную — это сотни строк работы с буферами.
    implementation("androidx.media3:media3-transformer:1.9.0")
    implementation("androidx.media3:media3-effect:1.9.0")
    // Поворот снимка живёт в EXIF: без него перекодированное фото ляжет набок.
    implementation("androidx.exifinterface:exifinterface:1.4.1")
    debugImplementation("androidx.compose.ui:ui-tooling")
}
