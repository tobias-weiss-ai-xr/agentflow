using UnityEngine;

namespace LearningWorlds
{
    public class LearningWorldsApp : MonoBehaviour
    {
        [RuntimeInitializeOnLoadMethod(RuntimeInitializeLoadType.SubsystemRegistration)]
        private static void Initialize ()
        {
            // Ensure the app rig exists and is persistent
            var instance = GameObject.Find("LearningWorldsAppRig");
            if (instance == null)
            {
                instance = new GameObject("LearningWorldsAppRig");
                DontDestroyOnLoad(instance);
            }
        }
    }
}