pluginManagement {
    repositories {
        google()
        mavenCentral()
        gradlePluginPortal()
    }
}

dependencyResolutionManagement {
    repositoriesMode.set(RepositoriesMode.FAIL_ON_PROJECT_REPOS)
    repositories {
        google()
        mavenCentral()
    }
}

check(System.getenv("GITHUB_ACTIONS") == "true") {
    "HostNR builds run only in GitHub Actions. Use the Android CI workflow."
}

rootProject.name = "HostNR"
include(":app")
project(":app").projectDir = file("android/app")
