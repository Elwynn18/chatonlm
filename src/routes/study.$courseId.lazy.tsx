// Page d'un cours (onglet Study), chargée à la première visite : plan,
// préparation, lecteur et fin selon l'état du cours.

import { createLazyFileRoute } from "@tanstack/react-router";
import { CoursePage } from "@/components/study/CoursePage";

export const Route = createLazyFileRoute("/study/$courseId")({
  component: CourseRoute,
});

function CourseRoute() {
  const { courseId } = Route.useParams();
  // Une page par cours : rien ne reste d'un cours à l'autre.
  return <CoursePage key={courseId} courseId={courseId} />;
}
